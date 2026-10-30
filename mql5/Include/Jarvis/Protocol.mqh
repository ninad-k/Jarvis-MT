//+------------------------------------------------------------------+
//| Protocol.mqh - request envelope parsing, dispatch, reply build.  |
//|                                                                  |
//| Request: {"id": str, "ts": ms, "cmd": str, "args": object}       |
//| Reply:   {"id","ok":true,"data":{...},"retcode":0,"ts":ms}       |
//|          {"id","ok":false,"error":"...","retcode":N,"ts":ms}     |
//| Both travel as two frames [hex HMAC, payload] (see Auth.mqh).    |
//|                                                                  |
//| Jarvis_HandleSigned() is the entry point of the EA: it checks the|
//| frames, the MAC, the timestamp and the id BEFORE anything is     |
//| parsed or executed, then calls Jarvis_Process().                 |
//| Both ALWAYS produce exactly one reply string, for any input,     |
//| because a ZeroMQ REP socket must answer every request.           |
//| Wire format: crates/bridge-client/src/protocol.rs                |
//+------------------------------------------------------------------+
#ifndef JARVIS_PROTOCOL_MQH
#define JARVIS_PROTOCOL_MQH

#include "Auth.mqh"
#include "Json.mqh"
#include "ReadHandlers.mqh"
#include "TradeHandlers.mqh"

#define JARVIS_MAX_ID_LEN     128
#define JARVIS_MAX_ERROR_LEN  240

//--- outcome of one request, used for the reply and for logging
struct JarvisResult
  {
   string            id;
   string            cmd;
   bool              ok;
   int               retcode;
   string            reply;
   bool              cached;   // a write answered from the reply cache (not executed again)
  };

string Jarvis_ErrorReply(const string id,const string message,const int retcode)
  {
   string msg=message;
   if(StringLen(msg)>JARVIS_MAX_ERROR_LEN)
      msg=StringSubstr(msg,0,JARVIS_MAX_ERROR_LEN);
   CJsonWriter w;
   w.BeginObject();
   w.PutStr("id",id);
   w.PutBool("ok",false);
   w.PutStr("error",msg);
   w.PutInt("retcode",retcode);
   w.PutInt("ts",Jarvis_NowMs());
   w.EndObject();
   return w.ToString();
  }

string Jarvis_OkReply(const string id,const string dataJson)
  {
   CJsonWriter w;
   w.BeginObject();
   w.PutStr("id",id);
   w.PutBool("ok",true);
   w.PutRaw("data",dataJson);
   w.PutInt("retcode",0);
   w.PutInt("ts",Jarvis_NowMs());
   w.EndObject();
   return w.ToString();
  }

//--- success reply of a write command: the envelope retcode is the trade retcode
string Jarvis_TradeOkReply(const string id,const int retcode,const string dataJson)
  {
   CJsonWriter w;
   w.BeginObject();
   w.PutStr("id",id);
   w.PutBool("ok",true);
   w.PutRaw("data",dataJson);
   w.PutInt("retcode",retcode);
   w.PutInt("ts",Jarvis_NowMs());
   w.EndObject();
   return w.ToString();
  }

//--- record a failure in r and build the error reply
void Jarvis_SetError(JarvisResult &r,const string message,const int retcode)
  {
   r.ok=false;
   r.retcode=retcode;
   r.reply=Jarvis_ErrorReply(r.id,message,retcode);
  }

bool Jarvis_IsWriteCommand(const string cmd)
  {
   return cmd=="order_send" || cmd=="position_modify" || cmd=="position_close" ||
          cmd=="position_close_partial" || cmd=="order_cancel";
  }

//+------------------------------------------------------------------+
//| Execute (or replay) one write command. r.id and r.cmd are set.  |
//| Every write reply, including refusals and validation errors, is  |
//| cached by request id; a repeated id returns the cached reply     |
//| verbatim and does not touch the trade server again.              |
//+------------------------------------------------------------------+
void Jarvis_Write(const string cmd,CJson &req,const int args,JarvisResult &r)
  {
   int hit=Jarvis_CacheFind(r.id);
   if(hit>=0)
     {
      g_jarvisCacheHits++;
      r.reply=g_jarvisCacheReply[hit];
      r.ok=g_jarvisCacheOk[hit];
      r.retcode=g_jarvisCacheRc[hit];
      r.cached=true;
      PrintFormat("JarvisBridge: duplicate write id=%s cmd=%s: returned the cached reply, nothing executed",
                  StringSubstr(r.id,0,36),cmd);
      return;
     }
   JarvisTradeOut out;
   Jarvis_WriteCommand(cmd,req,args,out);
   r.ok=out.ok;
   r.retcode=out.retcode;
   if(out.ok)
      r.reply=Jarvis_TradeOkReply(r.id,out.retcode,out.data);
   else
      r.reply=Jarvis_ErrorReply(r.id,out.error,out.retcode);
   Jarvis_CacheStore(r.id,r.reply,r.ok,r.retcode);
   PrintFormat("JarvisBridge: write cmd=%s id=%s ok=%s retcode=%d%s%s",cmd,StringSubstr(r.id,0,36),
               r.ok?"true":"false",r.retcode,r.ok?"":" error=",r.ok?"":out.error);
  }

//--- requests refused by authentication since start, and a throttled log line
ulong g_jarvisAuthRefused=0;
ulong g_jarvisAuthLastLog=0;
int   g_jarvisAuthSinceLog=0;

string Jarvis_FreshReason(const int why)
  {
   switch(why)
     {
      case JARVIS_FRESH_STALE:
         return "timestamp more than 30 s away from this PC's clock (is the clock right on both sides?)";
      case JARVIS_FRESH_REPLAYED:
         return "request id already used in the last 60 s";
      case JARVIS_FRESH_FULL:
         return "more than 4096 requests in the last 60 s";
      case JARVIS_FRESH_NO_TS:
         return "missing or invalid ts";
     }
   return "unknown";
  }

//--- count a refused request; log the first one and then at most one line per 10 s.
//--- Only a fixed reason text is logged, never anything the peer sent.
void Jarvis_AuthRefused(const string reason)
  {
   g_jarvisAuthRefused++;
   g_jarvisAuthSinceLog++;
   ulong now=Jarvis_MonoMs();
   if(g_jarvisAuthRefused==1 || (long)(now-g_jarvisAuthLastLog)>=10000)
     {
      PrintFormat("JarvisBridge: request refused (%s); %d refused since the last line, %I64u in total. "+
                  "Nothing was executed.",reason,g_jarvisAuthSinceLog,g_jarvisAuthRefused);
      g_jarvisAuthLastLog=now;
      g_jarvisAuthSinceLog=0;
     }
  }

//+------------------------------------------------------------------+
//| Handle one raw request text and fill r (r.reply is always set).  |
//| checkFresh: also require a fresh "ts" and a never-seen "id"      |
//| (the EA always passes true; the offline self-test of the handlers|
//| passes false). The caller has already verified the MAC.          |
//+------------------------------------------------------------------+
void Jarvis_Process(const string raw,JarvisResult &r,const bool checkFresh)
  {
   r.id="";
   r.cmd="";
   r.ok=false;
   r.retcode=JARVIS_RC_INVALID;
   r.reply="";
   r.cached=false;
   ResetLastError();

   CJson req;
   if(!req.Parse(raw))
     {
      Jarvis_SetError(r,req.Error(),JARVIS_RC_INVALID);
      return;
     }
   int root=req.Root();
   if(!req.IsObject(root))
     {
      Jarvis_SetError(r,"request must be a JSON object",JARVIS_RC_INVALID);
      return;
     }

   //--- id: required non-empty string
   int idNode=req.Get(root,"id");
   if(idNode<0 || !req.IsString(idNode) || req.Str(idNode)=="")
     {
      Jarvis_SetError(r,"missing or invalid 'id' (must be a non-empty string)",JARVIS_RC_INVALID);
      return;
     }
   string id=req.Str(idNode);
   if(StringLen(id)>JARVIS_MAX_ID_LEN)
     {
      Jarvis_SetError(r,"'id' is too long",JARVIS_RC_INVALID);
      return;
     }
   r.id=id;

   //--- freshness and replay: the sender holds the key, so its id may be echoed. Nothing below
   //--- (command, arguments, cache, trade handlers) runs for a stale or repeated request.
   if(checkFresh)
     {
      int tsNode=req.Get(root,"ts");
      int fresh=JARVIS_FRESH_NO_TS;
      if(tsNode>=0 && req.IsWhole(tsNode))
         fresh=Jarvis_FreshnessCheck(id,req.Int(tsNode));
      if(fresh!=JARVIS_FRESH_OK)
        {
         Jarvis_AuthRefused(Jarvis_FreshReason(fresh));
         Jarvis_SetError(r,JARVIS_AUTH_ERROR,JARVIS_RC_AUTH_FAILED);
         return;
        }
     }

   //--- cmd: required string
   int cmdNode=req.Get(root,"cmd");
   if(cmdNode<0 || !req.IsString(cmdNode) || req.Str(cmdNode)=="")
     {
      Jarvis_SetError(r,"missing or invalid 'cmd' (must be a non-empty string)",JARVIS_RC_INVALID);
      return;
     }
   string cmd=req.Str(cmdNode);
   if(StringLen(cmd)>64)
     {
      Jarvis_SetError(r,"unknown command",JARVIS_RC_UNKNOWN_CMD);
      return;
     }
   r.cmd=cmd;

   //--- args: optional object (missing or null => {})
   int args=req.Get(root,"args");
   if(args>=0 && req.Type(args)==JV_NULL)
      args=-1;
   if(args>=0 && !req.IsObject(args))
     {
      Jarvis_SetError(r,"'args' must be an object",JARVIS_RC_INVALID);
      return;
     }

   //--- write commands: answered from the reply cache when the id was seen before,
   //--- otherwise executed once and cached
   if(Jarvis_IsWriteCommand(cmd))
     {
      Jarvis_Write(cmd,req,args,r);
      return;
     }

   //--- dispatch
   CJsonWriter data;
   string err="";
   int rc;
   if(cmd=="ping")
      rc=Jarvis_CmdPing(req,args,data,err);
   else
      if(cmd=="get_price")
         rc=Jarvis_CmdGetPrice(req,args,data,err);
      else
         if(cmd=="get_account")
            rc=Jarvis_CmdGetAccount(req,args,data,err);
         else
            if(cmd=="get_positions")
               rc=Jarvis_CmdGetPositions(req,args,data,err);
            else
               if(cmd=="get_orders")
                  rc=Jarvis_CmdGetOrders(req,args,data,err);
               else
                  if(cmd=="get_history")
                     rc=Jarvis_CmdGetHistory(req,args,data,err);
                  else
                     if(cmd=="get_symbol")
                        rc=Jarvis_CmdGetSymbol(req,args,data,err);
                     else
                       {
                        Jarvis_SetError(r,"unknown command: "+cmd,JARVIS_RC_UNKNOWN_CMD);
                        return;
                       }

   if(rc!=0)
     {
      if(err=="")
         err="request failed";
      Jarvis_SetError(r,err,rc);
      return;
     }
   r.ok=true;
   r.retcode=0;
   r.reply=Jarvis_OkReply(id,data.ToString());
  }

//+------------------------------------------------------------------+
//| Entry point of the EA for one received message.                  |
//|   frameCount: how many frames the message had                    |
//|   macFrame, payload: frame 1 and frame 2 (empty if absent)       |
//| Anything but exactly two frames with a correct MAC is answered   |
//| with the generic "authentication failed" (-4) and an empty id,   |
//| so the reply reveals nothing and cannot be used to get a chosen  |
//| text signed. Nothing is parsed, cached or executed for it.       |
//+------------------------------------------------------------------+
void Jarvis_HandleSigned(const int frameCount,const uchar &macFrame[],const uchar &payload[],JarvisResult &r)
  {
   r.id="";
   r.cmd="";
   r.ok=false;
   r.retcode=JARVIS_RC_AUTH_FAILED;
   r.reply="";
   r.cached=false;

   if(!g_jarvisKeyReady)
     {
      Jarvis_AuthRefused("no key loaded");
      r.reply=Jarvis_ErrorReply("",JARVIS_AUTH_ERROR,JARVIS_RC_AUTH_FAILED);
      return;
     }
   if(frameCount!=2)
     {
      Jarvis_AuthRefused("not a two-frame message");
      r.reply=Jarvis_ErrorReply("",JARVIS_AUTH_ERROR,JARVIS_RC_AUTH_FAILED);
      return;
     }
   if(!Jarvis_VerifyFrames(macFrame,payload))
     {
      Jarvis_AuthRefused("MAC does not match (wrong key or altered message)");
      r.reply=Jarvis_ErrorReply("",JARVIS_AUTH_ERROR,JARVIS_RC_AUTH_FAILED);
      return;
     }
   //--- authentic. A zero byte cannot be part of valid JSON and would cut the text short.
   int n=ArraySize(payload);
   for(int i=0; i<n; i++)
      if(payload[i]==0)
        {
         Jarvis_SetError(r,"malformed request: contains a zero byte",JARVIS_RC_INVALID);
         return;
        }
   string text="";
   if(n>0)
      text=CharArrayToString(payload,0,n,CP_UTF8);
   Jarvis_Process(text,r,true);
   if(r.reply=="")   // defensive: never leave the client without an answer
      r.reply=Jarvis_ErrorReply(r.id,"internal error: empty reply",JARVIS_RC_INTERNAL);
  }

#endif // JARVIS_PROTOCOL_MQH
