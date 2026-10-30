//+------------------------------------------------------------------+
//| JarvisBridge.mq5                                                 |
//| Jarvis MT - localhost ZeroMQ bridge (Sprint 1: READ-ONLY).       |
//|                                                                  |
//| - ZeroMQ REP socket, bound to 127.0.0.1 only.                    |
//| - Polled with ZMQ_DONTWAIT from OnTimer (20 ms), at most 10      |
//|   requests per tick. Never blocks, never trades, no OnTick logic.|
//| - Exactly one reply for every request received.                  |
//| Wire format: crates/bridge-client/src/protocol.rs                |
//|                                                                  |
//| Requires "Allow DLL imports" for this EA (libzmq.dll).           |
//+------------------------------------------------------------------+
#property copyright   "Jarvis MT"
#property link        "https://github.com/dingmaotu/mql-zmq"
#property description "Read-only localhost bridge for the Jarvis MT assistant."
#property description "Binds a ZeroMQ REP socket on 127.0.0.1. Never places trades."

#include <Zmq/Zmq.mqh>
#include <Jarvis/Protocol.mqh>

input string InpEndpoint = "tcp://127.0.0.1:5555"; // ZeroMQ endpoint (must be tcp://127.0.0.1:<port>)
input bool   InpVerbose  = false;                  // Log one line per request

#define BRIDGE_TIMER_MS          20
#define BRIDGE_MAX_PER_TICK      10
#define BRIDGE_MAX_REQUEST_BYTES 65536
#define BRIDGE_EAGAIN            11        // libzmq/MSVC errno for "nothing to receive yet"
#define BRIDGE_PENDING_RETRIES   200       // 200 ticks * 20 ms = 4 s before the socket is rebuilt
#define BRIDGE_REBIND_INTERVAL   250       // ms between bind attempts after a rebuild

Context *g_ctx=NULL;
Socket  *g_sock=NULL;
string   g_hostPort="";          // "127.0.0.1:5555" for log lines
bool     g_hasPending=false;     // a reply is owed to the client but could not be sent yet
string   g_pendingReply="";
int      g_pendingTries=0;
uint     g_lastBindTry=0;
ulong    g_served=0;
uint     g_recvErrors=0;         // consecutive unexpected recv failures (log throttling)

//+------------------------------------------------------------------+
//| endpoint must be exactly tcp://127.0.0.1:<1..65535>              |
//+------------------------------------------------------------------+
bool EndpointIsLocalOnly(const string ep)
  {
   const string prefix="tcp://127.0.0.1:";
   if(StringFind(ep,prefix)!=0)
      return false;
   string port=StringSubstr(ep,StringLen(prefix));
   int n=StringLen(port);
   if(n<1 || n>5)
      return false;
   for(int i=0; i<n; i++)
     {
      ushort c=StringGetCharacter(port,i);
      if(c<'0' || c>'9')
         return false;
     }
   long p=StringToInteger(port);
   return p>=1 && p<=65535;
  }

//+------------------------------------------------------------------+
//| (Re)create the REP socket and bind it                            |
//+------------------------------------------------------------------+
bool OpenSocket()
  {
   if(CheckPointer(g_ctx)!=POINTER_DYNAMIC)
      return false;
   g_sock=new Socket(g_ctx,ZMQ_REP);
   if(CheckPointer(g_sock)!=POINTER_DYNAMIC)
     {
      g_sock=NULL;
      return false;
     }
   if(!g_sock.valid())
     {
      PrintFormat("JarvisBridge: zmq_socket failed: %s",Zmq::errorMessage());
      delete g_sock;
      g_sock=NULL;
      return false;
     }
   g_sock.setLinger(0);
   g_sock.setMaxMessageSize(BRIDGE_MAX_REQUEST_BYTES);
   if(!g_sock.bind(InpEndpoint))
     {
      PrintFormat("JarvisBridge: bind(%s) failed: %s",InpEndpoint,Zmq::errorMessage());
      delete g_sock;
      g_sock=NULL;
      return false;
     }
   g_hasPending=false;
   g_pendingReply="";
   g_pendingTries=0;
   return true;
  }

void CloseSocket()
  {
   if(CheckPointer(g_sock)==POINTER_DYNAMIC)
      delete g_sock;
   g_sock=NULL;
  }

//+------------------------------------------------------------------+
int OnInit()
  {
   if(!EndpointIsLocalOnly(InpEndpoint))
     {
      PrintFormat("JarvisBridge: refusing InpEndpoint '%s': only tcp://127.0.0.1:<port> is allowed (localhost only).",InpEndpoint);
      return INIT_PARAMETERS_INCORRECT;
     }
   if(MQLInfoInteger(MQL_TESTER)!=0)
     {
      Print("JarvisBridge: not supported in the strategy tester.");
      return INIT_FAILED;
     }
   if(MQLInfoInteger(MQL_DLLS_ALLOWED)==0)
     {
      Print("JarvisBridge: 'Allow DLL imports' is required for this EA (libzmq.dll).");
      return INIT_FAILED;
     }

   g_ctx=new Context();
   if(CheckPointer(g_ctx)!=POINTER_DYNAMIC || g_ctx.ref()==0)
     {
      Print("JarvisBridge: zmq_ctx_new failed.");
      if(CheckPointer(g_ctx)==POINTER_DYNAMIC)
         delete g_ctx;
      g_ctx=NULL;
      return INIT_FAILED;
     }
   if(!OpenSocket())
     {
      delete g_ctx;
      g_ctx=NULL;
      return INIT_FAILED;
     }
   if(!EventSetMillisecondTimer(BRIDGE_TIMER_MS))
     {
      Print("JarvisBridge: EventSetMillisecondTimer failed.");
      CloseSocket();
      delete g_ctx;
      g_ctx=NULL;
      return INIT_FAILED;
     }

   g_hostPort=StringSubstr(InpEndpoint,6);   // strip "tcp://"
   PrintFormat("JarvisBridge %s listening on %s (libzmq %s, read-only)",JARVIS_EA_VERSION,g_hostPort,Zmq::getVersion());
   return INIT_SUCCEEDED;
  }

//+------------------------------------------------------------------+
void OnDeinit(const int reason)
  {
   EventKillTimer();
   CloseSocket();                       // linger 0: drops unsent data, never blocks
   if(CheckPointer(g_ctx)==POINTER_DYNAMIC)
     {
      delete g_ctx;                     // zmq_ctx_term; all sockets are closed by now
      g_ctx=NULL;
     }
   PrintFormat("JarvisBridge stopped (reason %d, %I64u requests served).",reason,g_served);
  }

//+------------------------------------------------------------------+
//| Send a reply without blocking. If the socket cannot take it now, |
//| keep it and retry on the next tick: a REP socket cannot receive  |
//| again until its reply was sent.                                  |
//+------------------------------------------------------------------+
bool SendReply(const string reply)
  {
   if(CheckPointer(g_sock)!=POINTER_DYNAMIC)
      return false;
   if(g_sock.send(reply,true))
     {
      g_hasPending=false;
      g_pendingReply="";
      g_pendingTries=0;
      return true;
     }
   g_hasPending=true;
   g_pendingReply=reply;
   g_pendingTries=0;
   return false;
  }

//--- true when it is fine to receive again
bool ServicePendingReply()
  {
   if(!g_hasPending)
      return true;
   if(g_sock.send(g_pendingReply,true))
     {
      g_hasPending=false;
      g_pendingReply="";
      g_pendingTries=0;
      return true;
     }
   g_pendingTries++;
   if(g_pendingTries>=BRIDGE_PENDING_RETRIES)
     {
      PrintFormat("JarvisBridge: reply could not be sent (%s); rebuilding the socket.",Zmq::errorMessage());
      CloseSocket();
      g_hasPending=false;
      g_pendingReply="";
      g_pendingTries=0;
     }
   return false;
  }

//+------------------------------------------------------------------+
//| Receive one request (non-blocking), answer it exactly once.      |
//| returns false when there was nothing to do                       |
//+------------------------------------------------------------------+
bool HandleOne()
  {
   ZmqMsg msg;
   if(!g_sock.recv(msg,true))
     {
      int e=Zmq::errorNumber();
      if(e!=BRIDGE_EAGAIN && e!=0)
        {
         g_recvErrors++;
         if(g_recvErrors==1 || g_recvErrors%500==0)
            PrintFormat("JarvisBridge: recv failed: %s (errno %d, %u in a row)",Zmq::errorMessage(e),e,g_recvErrors);
        }
      return false;
     }
   g_recvErrors=0;

   ulong t0=GetMicrosecondCount();
   string text="";
   if(msg.size()>0)
      text=msg.getData();

   //--- a multipart request is still ONE request: drain the other frames, answer once
   bool more=msg.more();
   int guard=0;
   while(more && guard<16)
     {
      ZmqMsg part;
      if(!g_sock.recv(part,true))
         break;
      more=part.more();
      guard++;
     }

   JarvisResult r;
   Jarvis_Process(text,r);
   if(r.reply=="")   // defensive: never leave the client without an answer
      r.reply=Jarvis_ErrorReply(r.id,"internal error: empty reply",JARVIS_RC_INTERNAL);

   bool sent=SendReply(r.reply);
   g_served++;
   if(InpVerbose)
     {
      double ms=(double)(GetMicrosecondCount()-t0)/1000.0;
      string idShort=r.id;
      if(StringLen(idShort)>36)
         idShort=StringSubstr(idShort,0,36);
      PrintFormat("JarvisBridge: cmd=%s id=%s ok=%s retcode=%d %.2f ms%s",
                  (r.cmd=="")?"-":r.cmd,idShort,r.ok?"true":"false",r.retcode,ms,sent?"":" (reply queued)");
     }
   return true;
  }

//+------------------------------------------------------------------+
void OnTimer()
  {
   if(CheckPointer(g_sock)!=POINTER_DYNAMIC)
     {
      //--- socket was dropped after a send failure: bind it again, slowly
      uint now=GetTickCount();
      if(CheckPointer(g_ctx)==POINTER_DYNAMIC && (uint)(now-g_lastBindTry)>=BRIDGE_REBIND_INTERVAL)
        {
         g_lastBindTry=now;
         if(OpenSocket())
            PrintFormat("JarvisBridge: socket rebuilt, listening on %s",g_hostPort);
        }
      return;
     }
   if(!ServicePendingReply())
      return;
   for(int i=0; i<BRIDGE_MAX_PER_TICK; i++)
     {
      if(CheckPointer(g_sock)!=POINTER_DYNAMIC)
         break;
      if(!HandleOne())
         break;
      if(g_hasPending)
         break;   // cannot take another request until this reply is out
     }
  }
//+------------------------------------------------------------------+
