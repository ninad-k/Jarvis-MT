//+------------------------------------------------------------------+
//| JarvisBridge.mq5                                                 |
//| Jarvis MT - localhost ZeroMQ bridge.                             |
//|                                                                  |
//| - ZeroMQ REP socket, bound to 127.0.0.1 only.                    |
//| - Polled with ZMQ_DONTWAIT from OnTimer (20 ms), at most 10      |
//|   requests per tick. No OnTick logic.                            |
//| - Trades ONLY in response to a request (order_send, ...), never  |
//|   on its own. Real accounts are refused unless InpAllowReal.     |
//|   An order_send above InpMaxLot lots is refused (0 = no cap).    |
//| - Exactly one reply for every request received.                  |
//| - Authenticated (protocol 2): every message is two frames        |
//|   [hex HMAC-SHA256, JSON], keyed by the shared secret in         |
//|   MQL5\Files\JarvisMT\bridge.key. A wrong or missing MAC, a stale|
//|   ts or a reused id is refused with retcode -4 and nothing is    |
//|   executed. Without a usable key file the EA does not start.     |
//| - get_broker / list_symbols (0.4.0) read the broker's identity   |
//|   and its full symbol catalog; they never change Market Watch.   |
//| Wire format: crates/bridge-client/src/protocol.rs                |
//|                                                                  |
//| Requires "Allow DLL imports" for this EA (libzmq.dll).           |
//+------------------------------------------------------------------+
#property copyright   "Jarvis MT"
#property link        "https://github.com/dingmaotu/mql-zmq"
#property description "Localhost bridge for the Jarvis MT assistant."
#property description "Binds a ZeroMQ REP socket on 127.0.0.1. Only a holder of the shared key (bridge.key) can talk to it. Trades only on request, refuses real accounts unless InpAllowReal is set, and refuses orders above InpMaxLot lots."

#include <Zmq/Zmq.mqh>
#include <Jarvis/Protocol.mqh>

input string InpEndpoint = "tcp://127.0.0.1:5555"; // ZeroMQ endpoint (must be tcp://127.0.0.1:<port>)
input bool   InpVerbose  = false;                  // Log one line per request
input bool   InpAllowReal = false;                 // Allow write commands on a REAL account (default: demo only)
input double InpMaxLot   = 1.0;                    // Largest volume of one order_send in lots (0 = no cap)

#define BRIDGE_TIMER_MS          20
#define BRIDGE_MAX_PER_TICK      10
#define BRIDGE_MAX_REQUEST_BYTES 65536
#define BRIDGE_EAGAIN            11        // libzmq/MSVC errno for "nothing to receive yet"
#define BRIDGE_PENDING_RETRIES   200       // 200 ticks * 20 ms = 4 s before the socket is rebuilt
#define BRIDGE_REBIND_INTERVAL   250       // ms between bind attempts after a rebuild
#define BRIDGE_MAX_FRAMES        64        // frames of one message that are read; more and the socket is rebuilt

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
   if(!MathIsValidNumber(InpMaxLot) || InpMaxLot<0.0)
     {
      PrintFormat("JarvisBridge: refusing InpMaxLot %s: it must be 0 (no cap) or a positive number of lots.",DoubleToString(InpMaxLot,8));
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

   //--- fail closed: without a usable key nobody could be authenticated, so do not listen at all
   string keyErr="";
   if(!Jarvis_LoadKeyFile(JARVIS_KEY_FILE,keyErr))
     {
      PrintFormat("JarvisBridge: %s. The EA will not start without it. Use the same file for [bridge] key_file of the API.",keyErr);
      return INIT_FAILED;
     }
   Jarvis_ReplayReset();

   g_jarvisAllowReal=InpAllowReal;
   g_jarvisMaxLot=InpMaxLot;
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
   PrintFormat("JarvisBridge %s listening on %s (protocol %d, authenticated, libzmq %s, writes only on request, real accounts %s, max order %s)",
               JARVIS_EA_VERSION,g_hostPort,JARVIS_PROTOCOL_VERSION,Zmq::getVersion(),
               InpAllowReal?"ALLOWED (InpAllowReal=true)":"refused",
               (InpMaxLot>0.0)?CJsonWriter::FormatNum(InpMaxLot)+" lots":"unlimited (InpMaxLot=0)");
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
   ArrayInitialize(g_jarvisKey,0);
   g_jarvisKeyReady=false;
   PrintFormat("JarvisBridge stopped (reason %d, %I64u requests served, %I64u refused).",reason,g_served,g_jarvisAuthRefused);
  }

//+------------------------------------------------------------------+
//| Sign and send a reply as two frames [hex HMAC, JSON], without    |
//| blocking. The MAC is computed over the exact bytes sent.         |
//|   0: sent                                                        |
//|   1: the socket cannot take it now; nothing was queued           |
//|   2: could not sign, or the second frame failed after the first  |
//|      was queued: the socket was closed (OnTimer rebinds it)      |
//+------------------------------------------------------------------+
int SendSigned(const string reply)
  {
   uchar body[];
   Jarvis_Utf8(reply,body);
   string mac=Jarvis_HmacHex(g_jarvisKey,body);
   if(ArraySize(body)==0 || StringLen(mac)!=64)
     {
      Print("JarvisBridge: could not sign a reply; rebuilding the socket.");
      CloseSocket();
      return 2;
     }
   ZmqMsg macMsg(mac);
   if(!g_sock.sendMore(macMsg,true))
      return 1;
   ZmqMsg bodyMsg((int)ArraySize(body));
   bodyMsg.setData(body);
   if(!g_sock.send(bodyMsg,true))
     {
      PrintFormat("JarvisBridge: second frame of a reply failed (%s); rebuilding the socket.",Zmq::errorMessage());
      CloseSocket();
      return 2;
     }
   return 0;
  }

//+------------------------------------------------------------------+
//| Send a reply. If the socket cannot take it now, keep it and retry|
//| on the next tick: a REP socket cannot receive again until its    |
//| reply was sent.                                                  |
//+------------------------------------------------------------------+
bool SendReply(const string reply)
  {
   if(CheckPointer(g_sock)!=POINTER_DYNAMIC)
      return false;
   int st=SendSigned(reply);
   if(st==0)
     {
      g_hasPending=false;
      g_pendingReply="";
      g_pendingTries=0;
      return true;
     }
   if(st==2)
     {
      g_hasPending=false;
      g_pendingReply="";
      g_pendingTries=0;
      return false;
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
   if(CheckPointer(g_sock)!=POINTER_DYNAMIC)
     {
      g_hasPending=false;
      g_pendingReply="";
      return false;
     }
   int st=SendSigned(g_pendingReply);
   if(st==0 || st==2)
     {
      g_hasPending=false;
      g_pendingReply="";
      g_pendingTries=0;
      return st==0;
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

//--- copy the bytes of a received frame
void CopyFrame(ZmqMsg &m,uchar &dst[])
  {
   int sz=(int)m.size();
   ArrayResize(dst,sz);
   if(sz>0)
      m.getData(dst);        // copies ArraySize(dst) bytes, which is exactly the frame size
  }

//+------------------------------------------------------------------+
//| Receive one message (non-blocking) and read ALL its frames.      |
//| false: nothing to read (or the socket was dropped)               |
//+------------------------------------------------------------------+
bool ReceiveMessage(int &count,uchar &f1[],uchar &f2[])
  {
   ArrayResize(f1,0);
   ArrayResize(f2,0);
   count=0;
   ZmqMsg first;
   if(!g_sock.recv(first,true))
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
   count=1;
   CopyFrame(first,f1);
   bool more=first.more();
   while(more)
     {
      if(count>=BRIDGE_MAX_FRAMES)
        {
         //--- a message with absurdly many frames: the rest cannot be drained safely, start over
         PrintFormat("JarvisBridge: a message with more than %d frames was dropped; rebuilding the socket.",BRIDGE_MAX_FRAMES);
         CloseSocket();
         return false;
        }
      ZmqMsg part;
      if(!g_sock.recv(part,true))
         break;
      count++;
      if(count==2)
         CopyFrame(part,f2);
      more=part.more();
     }
   return true;
  }

//+------------------------------------------------------------------+
//| Receive one request (non-blocking), answer it exactly once.      |
//| returns false when there was nothing to do                       |
//+------------------------------------------------------------------+
bool HandleOne()
  {
   int count=0;
   uchar f1[],f2[];
   if(!ReceiveMessage(count,f1,f2))
      return false;

   ulong t0=GetMicrosecondCount();
   JarvisResult r;
   Jarvis_HandleSigned(count,f1,f2,r);
   ArrayResize(f1,0);
   ArrayResize(f2,0);

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
   Jarvis_NowMs();      // keeps the sub-second anchor of the clock fresh (see Auth.mqh)
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
