//+------------------------------------------------------------------+
//| Protocol.mqh - request envelope parsing, dispatch, reply build.  |
//|                                                                  |
//| Request: {"id": str, "cmd": str, "args": object}                 |
//| Reply:   {"id","ok":true,"data":{...},"retcode":0}               |
//|          {"id","ok":false,"error":"...","retcode":N}             |
//|                                                                  |
//| Jarvis_Process() ALWAYS produces exactly one reply string, for   |
//| any input, because a ZeroMQ REP socket must answer every request.|
//| Wire format: crates/bridge-client/src/protocol.rs                |
//+------------------------------------------------------------------+
#ifndef JARVIS_PROTOCOL_MQH
#define JARVIS_PROTOCOL_MQH

#include "Json.mqh"
#include "ReadHandlers.mqh"

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
//| Handle one raw request text and fill r (r.reply is always set).  |
//+------------------------------------------------------------------+
void Jarvis_Process(const string raw,JarvisResult &r)
  {
   r.id="";
   r.cmd="";
   r.ok=false;
   r.retcode=JARVIS_RC_INVALID;
   r.reply="";
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

   //--- write commands arrive in a later EA version
   if(Jarvis_IsWriteCommand(cmd))
     {
      Jarvis_SetError(r,"not implemented in this EA version",JARVIS_RC_UNKNOWN_CMD);
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

#endif // JARVIS_PROTOCOL_MQH
