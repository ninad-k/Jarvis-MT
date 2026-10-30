//+------------------------------------------------------------------+
//| JarvisSelfTest.mq5                                               |
//| Offline checks of the JSON module and the request dispatcher.    |
//| No ZeroMQ, no DLLs, no trading: it calls Jarvis_Process() with   |
//| canned requests and prints PASS/FAIL to the Experts journal.     |
//| Live-data replies (ping, account, positions, ...) are printed so |
//| they can be compared with the terminal by eye.                   |
//+------------------------------------------------------------------+
#property copyright "Jarvis MT"
#property description "Self-test for the Jarvis bridge JSON and protocol code."
#property script_show_inputs

#include <Jarvis/Protocol.mqh>

input bool InpPrintReplies = true;   // Print live-data replies

int g_pass=0;
int g_fail=0;

void Check(const bool cond,const string name,const string detail="")
  {
   if(cond)
     {
      g_pass++;
      PrintFormat("PASS  %s",name);
     }
   else
     {
      g_fail++;
      PrintFormat("FAIL  %s %s",name,detail);
     }
  }

//--- run a request; verify the reply is valid JSON with the expected envelope
bool Run(const string request,JarvisResult &r,CJson &rep)
  {
   Jarvis_Process(request,r);
   if(!rep.Parse(r.reply))
     {
      PrintFormat("FAIL  reply is not valid JSON: %s | %s",rep.Error(),StringSubstr(r.reply,0,200));
      g_fail++;
      return false;
     }
   int root=rep.Root();
   bool shape=rep.IsObject(root) && rep.IsString(rep.Get(root,"id")) &&
              rep.Type(rep.Get(root,"ok"))==JV_BOOL && rep.IsWhole(rep.Get(root,"retcode"));
   if(!shape)
     {
      PrintFormat("FAIL  reply envelope malformed: %s",StringSubstr(r.reply,0,200));
      g_fail++;
      return false;
     }
   return true;
  }

void ExpectError(const string name,const string request,const int rc,const string expectId,const string errPart="")
  {
   JarvisResult r;
   CJson rep;
   if(!Run(request,r,rep))
      return;
   int root=rep.Root();
   bool ok=!rep.Bool(rep.Get(root,"ok")) && rep.Int(rep.Get(root,"retcode"))==rc &&
           rep.Str(rep.Get(root,"id"))==expectId && rep.Str(rep.Get(root,"error"))!="" &&
           StringFind(rep.Str(rep.Get(root,"error")),errPart)>=0 && rep.Get(root,"data")<0;
   Check(ok,name,r.reply);
  }

void ExpectOk(const string name,const string request,CJson &rep,const bool print)
  {
   JarvisResult r;
   if(!Run(request,r,rep))
      return;
   int root=rep.Root();
   bool ok=rep.Bool(rep.Get(root,"ok")) && rep.Int(rep.Get(root,"retcode"))==0 &&
           rep.IsObject(rep.Get(root,"data")) && rep.Get(root,"error")<0;
   Check(ok,name,StringSubstr(r.reply,0,300));
   if(print && InpPrintReplies)
      PrintFormat("      %s -> %s",name,StringSubstr(r.reply,0,700));
  }

void TestJson()
  {
   CJson j;
   Check(j.Parse("{\"a\":1,\"b\":[true,false,null,\"x\"],\"c\":{\"d\":-2.5e1}}"),"json: parses nested value");
   Check(j.Get(j.Root(),"a")>=0 && j.Int(j.Get(j.Root(),"a"))==1 && j.IsWhole(j.Get(j.Root(),"a")),"json: integer");
   Check(MathAbs(j.Num(j.Get(j.Get(j.Root(),"c"),"d"))+25.0)<1e-9,"json: exponent number");
   Check(j.Count(j.Get(j.Root(),"b"))==4,"json: array count");
   Check(j.Get(j.Root(),"zzz")==-1 && j.Str(-1)=="" && j.Int(999)==0,"json: accessors tolerate bad indexes");

   Check(j.Parse("{\"s\":\"a\\\"b\\\\c\\n\\u0041\\u00e9\"}") &&
         j.Str(j.Get(j.Root(),"s"))=="a\"b\\c\nA"+ShortToString(0xE9),"json: string escapes and \\u");
   Check(j.Parse("{\"a\":1,\"a\":2}") && j.Int(j.Get(j.Root(),"a"))==2,"json: last duplicate key wins");

   string bad[]={"","{","[1,","{\"a\"}","{\"a\":}","{'a':1}","[1 2]","{\"a\":1,}","tru","01","1.","-","\"abc",
                 "{\"a\":\"\\x\"}","{\"a\":\"\\u12\"}","{} x","nul","[1e999]","\"a\nb\""};
   bool allRejected=true;
   for(int i=0; i<ArraySize(bad); i++)
     {
      if(j.Parse(bad[i]))
        {
         allRejected=false;
         PrintFormat("      accepted bad input: %s",bad[i]);
        }
     }
   Check(allRejected,"json: malformed inputs rejected");

   string deep="";
   for(int i=0; i<40; i++)
      deep+="[";
   for(int i=0; i<40; i++)
      deep+="]";
   Check(!j.Parse(deep),"json: depth limit enforced");
   string wide="[";
   for(int i=0; i<5000; i++)
      wide+=(i?",1":"1");
   wide+="]";
   Check(!j.Parse(wide),"json: node limit enforced");

   //--- writer
   Check(CJsonWriter::FormatNum(0.1)=="0.1","writer: 0.1");
   Check(CJsonWriter::FormatNum(100.0)=="100","writer: 100");
   Check(CJsonWriter::FormatNum(-0.000000001)=="0","writer: tiny negative -> 0");
   Check(CJsonWriter::FormatNum(MathSqrt(-1.0))=="0","writer: NaN -> 0");
   Check(CJsonWriter::FormatNum(MathExp(1000.0))=="0","writer: inf -> 0");
   Check(CJsonWriter::FormatNum(2391.1,2)=="2391.10","writer: fixed digits");
   Check(CJsonWriter::FormatNum(1.23456789012,-1)=="1.23456789","writer: 8 decimals trimmed");
   Check(CJsonWriter::Escape("a\"b\\c\r\n\t"+ShortToString(1))=="a\\\"b\\\\c\\r\\n\\t\\u0001","writer: escapes control chars");

   CJsonWriter w;
   w.BeginObject();
   w.PutStr("k","v\"");
   w.Key("arr");
   w.BeginArray();
   w.BeginObject();
   w.PutInt("n",5);
   w.PutNumOrNull("sl",0.0);
   w.PutNumOrNull("tp",1.5,2);
   w.EndObject();
   w.BeginObject();
   w.EndObject();
   w.EndArray();
   w.PutBool("b",true);
   w.PutNull("z");
   w.EndObject();
   Check(w.ToString()=="{\"k\":\"v\\\"\",\"arr\":[{\"n\":5,\"sl\":null,\"tp\":1.50},{}],\"b\":true,\"z\":null}","writer: commas and nesting",w.ToString());
   CJson back;
   Check(back.Parse(w.ToString()),"writer: output re-parses");

   //--- large output uses the chunked path
   CJsonWriter big;
   big.BeginArray();
   for(int i=0; i<2000; i++)
      big.ValStr("item-"+IntegerToString(i));
   big.EndArray();
   Check(back.Parse(big.ToString()) && back.Count(back.Root())==2000,"writer: large output");
  }

void TestProtocol()
  {
   ExpectError("proto: empty request","",-1,"");
   ExpectError("proto: not json","hello",-1,"");
   ExpectError("proto: array root","[1,2]",-1,"");
   ExpectError("proto: missing id","{\"cmd\":\"ping\"}",-1,"","id");
   ExpectError("proto: id wrong type","{\"id\":5,\"cmd\":\"ping\"}",-1,"","id");
   ExpectError("proto: missing cmd","{\"id\":\"x1\"}",-1,"x1","cmd");
   ExpectError("proto: unknown command","{\"id\":\"x2\",\"cmd\":\"launch_missiles\"}",-2,"x2","unknown command");
   ExpectError("proto: args not object","{\"id\":\"x3\",\"cmd\":\"ping\",\"args\":[1]}",-1,"x3","args");
   string writes[]={"order_send","position_modify","position_close","position_close_partial","order_cancel"};
   for(int i=0; i<ArraySize(writes); i++)
      ExpectError("proto: write command "+writes[i]+" rejected","{\"id\":\"w"+IntegerToString(i)+"\",\"cmd\":\""+writes[i]+"\",\"args\":{}}",
                  -2,"w"+IntegerToString(i),"not implemented in this EA version");
   ExpectError("proto: get_price without symbol","{\"id\":\"p1\",\"cmd\":\"get_price\"}",-1,"p1","symbol");
   ExpectError("proto: get_price symbol wrong type","{\"id\":\"p2\",\"cmd\":\"get_price\",\"args\":{\"symbol\":7}}",-1,"p2","symbol");
   ExpectError("proto: get_price unknown symbol","{\"id\":\"p3\",\"cmd\":\"get_price\",\"args\":{\"symbol\":\"XAUUS_NOPE\"}}",4301,"p3","unknown symbol: XAUUS_NOPE");
   ExpectError("proto: get_positions unknown symbol","{\"id\":\"p4\",\"cmd\":\"get_positions\",\"args\":{\"symbol\":\"NOPE_X\"}}",4301,"p4","unknown symbol");
   ExpectError("proto: history missing from","{\"id\":\"h1\",\"cmd\":\"get_history\",\"args\":{\"to\":100}}",-1,"h1","from");
   ExpectError("proto: history missing to","{\"id\":\"h2\",\"cmd\":\"get_history\",\"args\":{\"from\":100}}",-1,"h2","to");
   ExpectError("proto: history from > to","{\"id\":\"h3\",\"cmd\":\"get_history\",\"args\":{\"from\":200,\"to\":100}}",-1,"h3","from");
   ExpectError("proto: history limit 501","{\"id\":\"h4\",\"cmd\":\"get_history\",\"args\":{\"from\":1,\"to\":100,\"limit\":501}}",-1,"h4","limit");
   ExpectError("proto: history limit 0","{\"id\":\"h5\",\"cmd\":\"get_history\",\"args\":{\"from\":1,\"to\":100,\"limit\":0}}",-1,"h5","limit");
   ExpectError("proto: history from string","{\"id\":\"h6\",\"cmd\":\"get_history\",\"args\":{\"from\":\"1\",\"to\":100}}",-1,"h6","from");
   ExpectError("proto: history fractional limit","{\"id\":\"h7\",\"cmd\":\"get_history\",\"args\":{\"from\":1,\"to\":100,\"limit\":1.5}}",-1,"h7","limit");

   //--- id with quotes, backslash, unicode and newline survives the round trip
   JarvisResult r;
   CJson rep;
   string weirdId="a\"b\\c"+ShortToString(0xE9)+"\n"+ShortToString(0x20AC);
   string req="{\"id\":\""+CJsonWriter::Escape(weirdId)+"\",\"cmd\":\"nope\"}";
   if(Run(req,r,rep))
      Check(rep.Str(rep.Get(rep.Root(),"id"))==weirdId,"proto: awkward id echoed exactly");
  }

void TestLive()
  {
   CJson rep;
   JarvisResult r;
   string tmode="";
   bool haveAccount=Jarvis_TradeMode(tmode);
   PrintFormat("      account present: %s (trade_mode %s)",haveAccount?"yes":"no",tmode);
   if(!haveAccount)
     {
      ExpectError("live: ping without account fails cleanly","{\"id\":\"l0\",\"cmd\":\"ping\"}",JARVIS_RC_INTERNAL,"l0");
      return;
     }
   ExpectOk("live: ping","{\"id\":\"l1\",\"cmd\":\"ping\"}",rep,true);
   int d=rep.Get(rep.Root(),"data");
   Check(rep.Int(rep.Get(d,"protocol_version"))==1 && rep.Str(rep.Get(d,"ea_version"))=="0.1.0" &&
         rep.Type(rep.Get(d,"terminal_connected"))==JV_BOOL && rep.Type(rep.Get(d,"trade_allowed"))==JV_BOOL &&
         rep.IsWhole(rep.Get(d,"server_time")),"live: ping fields");

   ExpectOk("live: get_account","{\"id\":\"l2\",\"cmd\":\"get_account\",\"args\":{}}",rep,true);
   d=rep.Get(rep.Root(),"data");
   Check(rep.Get(d,"login")<0 && rep.Get(d,"name")<0 && rep.Get(d,"server")<0 && rep.Count(d)==9,"live: account has no identity fields");

   ExpectOk("live: get_positions","{\"id\":\"l3\",\"cmd\":\"get_positions\",\"args\":{}}",rep,true);
   Check(rep.Count(rep.Get(rep.Get(rep.Root(),"data"),"positions"))==PositionsTotal(),"live: position count matches terminal");
   ExpectOk("live: get_orders","{\"id\":\"l4\",\"cmd\":\"get_orders\"}",rep,true);
   Check(rep.Count(rep.Get(rep.Get(rep.Root(),"data"),"orders"))==OrdersTotal(),"live: order count matches terminal");

   long now=(long)TimeTradeServer();
   string hreq=StringFormat("{\"id\":\"l5\",\"cmd\":\"get_history\",\"args\":{\"from\":%I64d,\"to\":%I64d,\"limit\":5}}",now-30*86400,now);
   ExpectOk("live: get_history (30 days, limit 5)",hreq,rep,true);
   d=rep.Get(rep.Root(),"data");
   int deals=rep.Get(d,"deals");
   bool sorted=true;
   for(int i=1; i<rep.Count(deals); i++)
      if(rep.Int(rep.Get(rep.At(deals,i),"time"))>rep.Int(rep.Get(rep.At(deals,i-1),"time")))
         sorted=false;
   Check(sorted && rep.Count(deals)<=5 && rep.Type(rep.Get(d,"truncated"))==JV_BOOL,"live: history newest first, limit respected");

   if(SymbolsTotal(true)>0)
     {
      string sym=SymbolName(0,true);
      ExpectOk("live: get_price "+sym,"{\"id\":\"l6\",\"cmd\":\"get_price\",\"args\":{\"symbol\":\""+CJsonWriter::Escape(sym)+"\"}}",rep,true);
     }
  }

void OnStart()
  {
   Print("=== JarvisSelfTest start ===");
   TestJson();
   TestProtocol();
   TestLive();
   PrintFormat("=== JarvisSelfTest done: %d passed, %d failed ===",g_pass,g_fail);
  }
