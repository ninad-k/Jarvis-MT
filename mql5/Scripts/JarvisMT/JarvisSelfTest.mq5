//+------------------------------------------------------------------+
//| JarvisSelfTest.mq5                                               |
//| Offline checks of the JSON module and the request dispatcher.    |
//| No ZeroMQ, no DLLs, no trades: it calls Jarvis_Process() with    |
//| canned requests and prints PASS/FAIL to the Experts journal.     |
//| The write-command tests send ONLY requests that must be rejected |
//| (bad arguments, unknown tickets, wrong-side stops, cached ids);  |
//| they run on demo accounts only, and the script verifies that the |
//| number of positions and orders did not change.                   |
//| Live-data replies (ping, account, positions, ...) are printed so |
//| they can be compared with the terminal by eye.                   |
//+------------------------------------------------------------------+
#property copyright "Jarvis MT"
#property description "Self-test for the Jarvis bridge JSON and protocol code."
#property script_show_inputs

#include <Jarvis/Protocol.mqh>

input bool InpPrintReplies = true;   // Print live-data replies

#define JARVIS_SELFTEST_MAGIC 770078

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
      ExpectError("proto: write command "+writes[i]+" without arguments is invalid",
                  "{\"id\":\"w"+IntegerToString(i)+"\",\"cmd\":\""+writes[i]+"\",\"args\":{}}",
                  -1,"w"+IntegerToString(i));
   ExpectError("proto: get_symbol without symbol","{\"id\":\"s1\",\"cmd\":\"get_symbol\"}",-1,"s1","symbol");
   ExpectError("proto: get_symbol unknown symbol","{\"id\":\"s2\",\"cmd\":\"get_symbol\",\"args\":{\"symbol\":\"NOPE_X\"}}",4301,"s2","unknown symbol");
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
   Check(rep.Int(rep.Get(d,"protocol_version"))==1 && rep.Str(rep.Get(d,"ea_version"))==JARVIS_EA_VERSION &&
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
      ExpectOk("live: get_symbol "+sym,"{\"id\":\"l7\",\"cmd\":\"get_symbol\",\"args\":{\"symbol\":\""+CJsonWriter::Escape(sym)+"\"}}",rep,true);
      d=rep.Get(rep.Root(),"data");
      Check(rep.Count(d)==13 && rep.Str(rep.Get(d,"symbol"))==sym && rep.Num(rep.Get(d,"volume_step"))>0.0 &&
            rep.Num(rep.Get(d,"volume_min"))>0.0 && rep.Num(rep.Get(d,"volume_max"))>=rep.Num(rep.Get(d,"volume_min")) &&
            rep.Num(rep.Get(d,"tick_size"))>0.0 && rep.Num(rep.Get(d,"point"))>0.0 && rep.IsWhole(rep.Get(d,"stops_level")) &&
            rep.Type(rep.Get(d,"trade_allowed"))==JV_BOOL && rep.Type(rep.Get(d,"description"))==JV_STRING &&
            rep.Type(rep.Get(d,"currency_profit"))==JV_STRING && rep.IsWhole(rep.Get(d,"digits")),"live: get_symbol fields");
     }
  }

//+------------------------------------------------------------------+
//| Write commands: requests that MUST be rejected. Nothing is traded|
//+------------------------------------------------------------------+
string BuildOrder(const string id,const string sym,const string side,const string type,const double vol,
                  const double price,const double sl,const double tp,const string comment="jarvis:selftest")
  {
   CJsonWriter w;
   w.BeginObject();
   w.PutStr("id",id);
   w.PutStr("cmd","order_send");
   w.Key("args");
   w.BeginObject();
   w.PutStr("symbol",sym);
   w.PutStr("side",side);
   w.PutNum("volume",vol);
   w.PutStr("type",type);
   if(price>=0.0)
      w.PutNum("price",price);
   if(sl>=0.0)
      w.PutNum("sl",sl);
   if(tp>=0.0)
      w.PutNum("tp",tp);
   w.PutInt("magic",JARVIS_SELFTEST_MAGIC);
   w.PutStr("comment",comment);
   w.EndObject();
   w.EndObject();
   return w.ToString();
  }

//--- a tradable symbol with quotes from Market Watch, "" if none
string FindTradableSymbol()
  {
   int total=SymbolsTotal(true);
   for(int i=0; i<total; i++)
     {
      string s=SymbolName(i,true);
      MqlTick t;
      if(SymbolInfoInteger(s,SYMBOL_TRADE_MODE)==SYMBOL_TRADE_MODE_FULL && SymbolInfoTick(s,t) && t.bid>0.0 && t.ask>0.0 &&
         SymbolInfoDouble(s,SYMBOL_VOLUME_MIN)>0.0 && SymbolInfoDouble(s,SYMBOL_VOLUME_STEP)>0.0)
         return s;
     }
   return "";
  }

void TestWrites()
  {
   //--- structurally invalid requests: rejected with -1 on every account, before the safety gate
   ExpectError("write: order_send bad side",BuildOrder("ws1","EURUSD","hold","market",0.01,-1,-1,-1),-1,"ws1","side");
   ExpectError("write: order_send bad type",BuildOrder("ws2","EURUSD","buy","fok",0.01,-1,-1,-1),-1,"ws2","type");
   ExpectError("write: order_send market with price",BuildOrder("ws3","EURUSD","buy","market",0.01,1.1,-1,-1),-1,"ws3","price");
   ExpectError("write: order_send limit without price",BuildOrder("ws4","EURUSD","buy","limit",0.01,-1,-1,-1),-1,"ws4","price");
   ExpectError("write: order_send comment too long",BuildOrder("ws5","EURUSD","buy","market",0.01,-1,-1,-1,"0123456789012345678901234567890X"),-1,"ws5","comment");
   ExpectError("write: order_send volume as string",
               "{\"id\":\"ws6\",\"cmd\":\"order_send\",\"args\":{\"symbol\":\"EURUSD\",\"side\":\"buy\",\"type\":\"market\",\"volume\":\"0.01\",\"magic\":1}}",
               -1,"ws6","volume");
   ExpectError("write: order_send without magic",
               "{\"id\":\"ws7\",\"cmd\":\"order_send\",\"args\":{\"symbol\":\"EURUSD\",\"side\":\"buy\",\"type\":\"market\",\"volume\":0.01}}",
               -1,"ws7","magic");
   ExpectError("write: position_modify without sl and tp","{\"id\":\"ws8\",\"cmd\":\"position_modify\",\"args\":{\"ticket\":5}}",-1,"ws8","sl");
   ExpectError("write: position_modify negative sl","{\"id\":\"ws9\",\"cmd\":\"position_modify\",\"args\":{\"ticket\":5,\"sl\":-1}}",-1,"ws9","negative");
   ExpectError("write: position_close ticket 0","{\"id\":\"ws10\",\"cmd\":\"position_close\",\"args\":{\"ticket\":0}}",-1,"ws10","ticket");
   ExpectError("write: order_cancel ticket as string","{\"id\":\"ws11\",\"cmd\":\"order_cancel\",\"args\":{\"ticket\":\"7\"}}",-1,"ws11","ticket");
   ExpectError("write: position_close_partial without volume","{\"id\":\"ws12\",\"cmd\":\"position_close_partial\",\"args\":{\"ticket\":5}}",-1,"ws12","volume");
   ExpectError("write: position_close bad deviation","{\"id\":\"ws13\",\"cmd\":\"position_close\",\"args\":{\"ticket\":5,\"deviation_points\":-1}}",-1,"ws13","deviation");

   //--- everything below depends on the account and the market: demo accounts only
   string mode="";
   if(!Jarvis_TradeMode(mode))
     {
      Print("      SKIP state-dependent write tests: no connected trading account");
      return;
     }
   if(mode!="demo")
     {
      PrintFormat("      SKIP state-dependent write tests: account trade mode is %s (demo required)",mode);
      return;
     }
   string gateErr="";
   int gate=Jarvis_WriteGate(gateErr);
   string sym=FindTradableSymbol();
   if(sym=="")
     {
      Print("      SKIP state-dependent write tests: no tradable symbol with quotes in Market Watch");
      return;
     }
   if(gate!=0)
     {
      PrintFormat("      safety gate is closed (%s): checking that well-formed writes are refused with -3",gateErr);
      ExpectError("write: well-formed order_send is refused by the gate",
                  BuildOrder("wg1",sym,"buy","market",SymbolInfoDouble(sym,SYMBOL_VOLUME_MIN),-1,-1,-1),JARVIS_RC_WRITE_REFUSED,"wg1","refused");
      ExpectError("write: position_close is refused by the gate","{\"id\":\"wg2\",\"cmd\":\"position_close\",\"args\":{\"ticket\":5}}",
                  JARVIS_RC_WRITE_REFUSED,"wg2","refused");
      return;
     }

   int posBefore=PositionsTotal();
   int ordBefore=OrdersTotal();
   MqlTick tk;
   SymbolInfoTick(sym,tk);
   double vmin=SymbolInfoDouble(sym,SYMBOL_VOLUME_MIN);
   double vmax=SymbolInfoDouble(sym,SYMBOL_VOLUME_MAX);
   double vstep=SymbolInfoDouble(sym,SYMBOL_VOLUME_STEP);
   double point=SymbolInfoDouble(sym,SYMBOL_POINT);
   int digits=Jarvis_Digits(sym);
   long stops=0;
   SymbolInfoInteger(sym,SYMBOL_TRADE_STOPS_LEVEL,stops);
   double gap=(double)(stops+100)*point;     // a distance that is certainly beyond the stops level
   double ask=tk.ask,bid=tk.bid;
   PrintFormat("      write tests on %s (bid %s ask %s, volume %s..%s step %s, stops level %I64d)",sym,
               DoubleToString(bid,digits),DoubleToString(ask,digits),CJsonWriter::FormatNum(vmin),
               CJsonWriter::FormatNum(vmax),CJsonWriter::FormatNum(vstep),stops);

   //--- volume
   ExpectError("write: volume 0",BuildOrder("wv1",sym,"buy","market",0.0,-1,-1,-1),-1,"wv1","volume");
   ExpectError("write: volume below minimum",BuildOrder("wv2",sym,"buy","market",vmin*0.5,-1,-1,-1),-1,"wv2","below the minimum");
   ExpectError("write: volume above maximum",BuildOrder("wv3",sym,"buy","market",vmax+vstep,-1,-1,-1),-1,"wv3","above the maximum");
   ExpectError("write: volume not a multiple of the step",BuildOrder("wv4",sym,"buy","market",vmin+vstep*0.5,-1,-1,-1),-1,"wv4","multiple of the volume step");
   ExpectError("write: unknown symbol",BuildOrder("wv5","NOPE_X","buy","market",vmin,-1,-1,-1),-1,"wv5","unknown symbol");

   //--- stops on the wrong side of the market
   ExpectError("write: buy with sl above the market",BuildOrder("wl1",sym,"buy","market",vmin,-1,NormalizeDouble(ask+gap,digits),-1),-1,"wl1","sl");
   ExpectError("write: buy with tp below the market",BuildOrder("wl2",sym,"buy","market",vmin,-1,-1,NormalizeDouble(bid-gap,digits)),-1,"wl2","tp");
   ExpectError("write: sell with sl below the market",BuildOrder("wl3",sym,"sell","market",vmin,-1,NormalizeDouble(bid-gap,digits),-1),-1,"wl3","sl");
   ExpectError("write: sell with tp above the market",BuildOrder("wl4",sym,"sell","market",vmin,-1,-1,NormalizeDouble(ask+gap,digits)),-1,"wl4","tp");
   ExpectError("write: buy with sl at the bid (inside the stops level)",BuildOrder("wl5",sym,"buy","market",vmin,-1,NormalizeDouble(bid,digits),-1),-1,"wl5","sl");
   ExpectError("write: price with too many decimals",BuildOrder("wl6",sym,"buy","limit",vmin,NormalizeDouble(bid-gap,digits)+point*0.1,-1,-1),-1,"wl6","decimals");

   //--- pending order prices on the wrong side
   ExpectError("write: buy limit above the ask",BuildOrder("wp1",sym,"buy","limit",vmin,NormalizeDouble(ask+gap,digits),-1,-1),-1,"wp1","below");
   ExpectError("write: buy stop below the ask",BuildOrder("wp2",sym,"buy","stop",vmin,NormalizeDouble(ask-gap,digits),-1,-1),-1,"wp2","above");
   ExpectError("write: sell limit below the bid",BuildOrder("wp3",sym,"sell","limit",vmin,NormalizeDouble(bid-gap,digits),-1,-1),-1,"wp3","above");
   ExpectError("write: sell stop above the bid",BuildOrder("wp4",sym,"sell","stop",vmin,NormalizeDouble(bid+gap,digits),-1,-1),-1,"wp4","below");
   double farBuy=NormalizeDouble(ask-gap*4.0,digits);
   ExpectError("write: buy limit with sl above its price",BuildOrder("wp5",sym,"buy","limit",vmin,farBuy,NormalizeDouble(farBuy+gap,digits),-1),-1,"wp5","sl");

   //--- unknown tickets
   ExpectError("write: position_modify unknown ticket","{\"id\":\"wt1\",\"cmd\":\"position_modify\",\"args\":{\"ticket\":999999999999,\"sl\":1.0}}",-1,"wt1","position not found");
   ExpectError("write: position_close unknown ticket","{\"id\":\"wt2\",\"cmd\":\"position_close\",\"args\":{\"ticket\":999999999999}}",-1,"wt2","position not found");
   ExpectError("write: position_close_partial unknown ticket","{\"id\":\"wt3\",\"cmd\":\"position_close_partial\",\"args\":{\"ticket\":999999999999,\"volume\":0.01}}",-1,"wt3","position not found");
   ExpectError("write: order_cancel unknown ticket","{\"id\":\"wt4\",\"cmd\":\"order_cancel\",\"args\":{\"ticket\":999999999999}}",-1,"wt4","pending order not found");

   //--- partial close volume must be less than the position volume (needs a position to look at)
   if(PositionsTotal()>0)
     {
      ulong pt=PositionGetTicket(0);
      double pvol=PositionGetDouble(POSITION_VOLUME);
      if(pt>0)
         ExpectError("write: partial close of the whole position volume",
                     StringFormat("{\"id\":\"wt5\",\"cmd\":\"position_close_partial\",\"args\":{\"ticket\":%I64u,\"volume\":%s}}",pt,CJsonWriter::FormatNum(pvol)),
                     -1,"wt5","less than the position volume");
      else
         Print("      SKIP partial-close test: could not read the first position");
     }
   else
      Print("      SKIP partial-close test: no open position");

   //--- reply cache: a repeated id returns the first reply verbatim and executes nothing
   JarvisResult r1,r2,r3;
   string firstReq=BuildOrder("dup-1",sym,"buy","market",0.0,-1,-1,-1);                 // rejected: volume 0
   Jarvis_Process(firstReq,r1);
   ulong hits=g_jarvisCacheHits;
   string otherReq="{\"id\":\"dup-1\",\"cmd\":\"position_close\",\"args\":{\"ticket\":999999999999}}";   // other command, same id
   Jarvis_Process(otherReq,r2);
   Check(!r1.ok && !r1.cached && r2.reply==r1.reply && r2.cached && g_jarvisCacheHits==hits+1,
         "write cache: a repeated id returns the cached reply, nothing runs again",r2.reply);
   Jarvis_Process(firstReq,r3);
   Check(r3.reply==r1.reply && r3.cached && g_jarvisCacheHits==hits+2,"write cache: still cached on the third request");
   //--- the ring holds 64 replies: after 64 other ids the first one is forgotten
   for(int i=0; i<JARVIS_CACHE_SIZE; i++)
     {
      JarvisResult rx;
      Jarvis_Process(StringFormat("{\"id\":\"ring-%d\",\"cmd\":\"order_cancel\",\"args\":{\"ticket\":999999999999}}",i),rx);
     }
   Jarvis_Process(otherReq,r2);
   Check(!r2.cached && r2.reply!=r1.reply,"write cache: the oldest reply is evicted after 64 newer ones");
   ulong h2=g_jarvisCacheHits;
   JarvisResult r4;
   Jarvis_Process("{\"id\":\"ring-63\",\"cmd\":\"order_cancel\",\"args\":{\"ticket\":999999999999}}",r4);
   Check(r4.cached && g_jarvisCacheHits==h2+1,"write cache: a recent id is still cached");

   Check(PositionsTotal()==posBefore && OrdersTotal()==ordBefore,"write: no position or order was created or changed by the rejected requests");
  }

void OnStart()
  {
   Print("=== JarvisSelfTest start ===");
   TestJson();
   TestProtocol();
   TestLive();
   TestWrites();
   PrintFormat("=== JarvisSelfTest done: %d passed, %d failed ===",g_pass,g_fail);
  }
