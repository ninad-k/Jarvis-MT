//+------------------------------------------------------------------+
//| JarvisSelfTest.mq5                                               |
//| Offline checks of the JSON module, the request dispatcher and the|
//| message authentication (HMAC-SHA256 against RFC 4231, the golden |
//| vector shared with the Rust tests, wrong key / replay / stale ts).|
//| No ZeroMQ, no DLLs, no trades: it calls Jarvis_Process() with    |
//| canned requests and prints PASS/FAIL to the Experts journal.     |
//| The write-command tests send ONLY requests that must be rejected |
//| (bad arguments, unknown tickets, wrong-side stops, cached ids);  |
//| they run on demo accounts only, and the script verifies that the |
//| number of positions and orders did not change.                   |
//| get_broker and list_symbols are checked for argument bounds,     |
//| paging, order, JSON escaping and caching; Market Watch must stay |
//| unchanged.                                                       |
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
   Jarvis_Process(request,r,false);
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
           (errPart=="" || StringFind(rep.Str(rep.Get(root,"error")),errPart)>=0) && rep.Get(root,"data")<0;
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
   Check(rep.Int(rep.Get(d,"protocol_version"))==JARVIS_PROTOCOL_VERSION && rep.Str(rep.Get(d,"ea_version"))==JARVIS_EA_VERSION &&
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
//| get_broker and list_symbols. No network, no trades, and nothing  |
//| is added to Market Watch (checked).                              |
//+------------------------------------------------------------------+
int CountOccurrences(const string text,const string needle)
  {
   int count=0,pos=0;
   while((pos=StringFind(text,needle,pos))>=0)
     {
      count++;
      pos+=StringLen(needle);
     }
   return count;
  }

//--- run list_symbols{offset,limit} and parse the reply; false (and a FAIL line) if it is not ok
bool ListPage(const string id,const long offset,const long limit,CJson &rep,int &data)
  {
   JarvisResult r;
   string req=StringFormat("{\"id\":\"%s\",\"cmd\":\"list_symbols\",\"args\":{\"offset\":%I64d,\"limit\":%I64d}}",id,offset,limit);
   if(!Run(req,r,rep))
      return false;
   int root=rep.Root();
   data=rep.Get(root,"data");
   bool ok=rep.Bool(rep.Get(root,"ok")) && rep.IsObject(data) && rep.IsWhole(rep.Get(data,"total")) &&
           rep.IsWhole(rep.Get(data,"offset")) && rep.Type(rep.Get(data,"symbols"))==JV_ARRAY;
   Check(ok,"catalog: page reply shape ("+id+")",StringSubstr(r.reply,0,300));
   return ok;
  }

void TestCatalogSort()
  {
   string a[]={"US100_Spot","XAUUSD_","EURUSD_","EURUSD","eurusd","A","Z","XAUUSD","EURUSD"};
   Check(Jarvis_SortNames(a,ArraySize(a)) && Jarvis_NamesSorted(a,ArraySize(a)),"catalog: sort orders names");
   Check(a[0]=="A" && StringCompare("EURUSD","EURUSD_",true)<0,"catalog: sort is ascending and case-sensitive");
   int pEur=-1,pEurU=-1;
   for(int i=0; i<ArraySize(a); i++)
     {
      if(a[i]=="EURUSD" && pEur<0)
         pEur=i;
      if(a[i]=="EURUSD_")
         pEurU=i;
     }
   Check(pEur>=0 && pEurU>pEur,"catalog: EURUSD sorts before EURUSD_");

   string empty[];
   Check(Jarvis_SortNames(empty,0) && Jarvis_NamesSorted(empty,0),"catalog: sort of an empty list");
   string one[]={"X"};
   Check(Jarvis_SortNames(one,1) && one[0]=="X","catalog: sort of one name");

   //--- 777 pseudo-random names with duplicates: sorted, and nothing lost or invented
   const int n=777;
   string names[],orig[];
   ArrayResize(names,n);
   ArrayResize(orig,n);
   uint x=12345;
   for(int i=0; i<n; i++)
     {
      x=x*1664525+1013904223;
      uint pick=(x>>24)%3;
      string suffix=(pick==0)?"_":((pick==1)?".pro":"");
      orig[i]="S"+IntegerToString((int)((x>>8)%400))+suffix;
      names[i]=orig[i];
     }
   bool sortedOk=Jarvis_SortNames(names,n) && Jarvis_NamesSorted(names,n);
   Check(sortedOk,"catalog: sort of 777 names");
   bool used[];
   ArrayResize(used,n);
   ArrayInitialize(used,false);
   bool permutation=true;
   for(int i=0; i<n && permutation; i++)
     {
      bool found=false;
      for(int j=0; j<n; j++)
         if(!used[j] && orig[j]==names[i])
           {
            used[j]=true;
            found=true;
            break;
           }
      permutation=found;
     }
   Check(permutation,"catalog: sort keeps every name exactly once");
  }

void TestCatalogEscaping()
  {
   //--- a path contains backslashes, a description may hold quotes and non-ASCII text
   string path="FX Gold Elite\\XAUUSD_";
   string desc="Gold \"spot\" vs US Dollar "+ShortToString(0x20AC)+"\t";
   CJsonWriter w;
   w.BeginObject();
   w.Key("symbols");
   w.BeginArray();
   Jarvis_PutCatalogEntry(w,"XAUUSD_",desc,path,"full",true,2,"XAU","USD");
   w.EndArray();
   w.PutInt("total",1);
   w.PutInt("offset",0);
   w.EndObject();
   string text=w.ToString();
   Check(StringFind(text,"FX Gold Elite\\\\XAUUSD_")>=0 && StringFind(text,"\\\"spot\\\"")>=0,"catalog: backslash and quote are escaped in the text",text);
   CJson j;
   bool parsed=j.Parse(text);
   Check(parsed,"catalog: escaped entry is valid JSON",j.Error());
   if(!parsed)
      return;
   int e=j.At(j.Get(j.Root(),"symbols"),0);
   Check(j.Str(j.Get(e,"path"))==path && j.Str(j.Get(e,"description"))==desc && j.Str(j.Get(e,"name"))=="XAUUSD_" &&
         j.Str(j.Get(e,"trade_mode"))=="full" && j.Bool(j.Get(e,"visible")) && j.Int(j.Get(e,"digits"))==2 &&
         j.Str(j.Get(e,"currency_base"))=="XAU" && j.Str(j.Get(e,"currency_profit"))=="USD" && j.Count(e)==8,
         "catalog: path with backslashes and awkward description survive the round trip");
   Check(Jarvis_SymbolTradeModeName(SYMBOL_TRADE_MODE_FULL)=="full" && Jarvis_SymbolTradeModeName(SYMBOL_TRADE_MODE_LONGONLY)=="long_only" &&
         Jarvis_SymbolTradeModeName(SYMBOL_TRADE_MODE_SHORTONLY)=="short_only" && Jarvis_SymbolTradeModeName(SYMBOL_TRADE_MODE_CLOSEONLY)=="close_only" &&
         Jarvis_SymbolTradeModeName(SYMBOL_TRADE_MODE_DISABLED)=="disabled" && Jarvis_SymbolTradeModeName(99)=="disabled",
         "catalog: trade modes map to the wire names");
  }

void TestCatalogArguments()
  {
   const string pre="{\"id\":\"";
   ExpectError("catalog: limit 0",pre+"c-l0\",\"cmd\":\"list_symbols\",\"args\":{\"limit\":0}}",-1,"c-l0","limit");
   ExpectError("catalog: limit 1001",pre+"c-l1\",\"cmd\":\"list_symbols\",\"args\":{\"limit\":1001}}",-1,"c-l1","limit");
   ExpectError("catalog: limit -5",pre+"c-l2\",\"cmd\":\"list_symbols\",\"args\":{\"limit\":-5}}",-1,"c-l2","limit");
   ExpectError("catalog: limit as string",pre+"c-l3\",\"cmd\":\"list_symbols\",\"args\":{\"limit\":\"10\"}}",-1,"c-l3","limit");
   ExpectError("catalog: fractional limit",pre+"c-l4\",\"cmd\":\"list_symbols\",\"args\":{\"limit\":1.5}}",-1,"c-l4","limit");
   ExpectError("catalog: offset -1",pre+"c-o1\",\"cmd\":\"list_symbols\",\"args\":{\"offset\":-1}}",-1,"c-o1","offset");
   ExpectError("catalog: offset as string",pre+"c-o2\",\"cmd\":\"list_symbols\",\"args\":{\"offset\":\"0\"}}",-1,"c-o2","offset");
   ExpectError("catalog: fractional offset",pre+"c-o3\",\"cmd\":\"list_symbols\",\"args\":{\"offset\":0.5}}",-1,"c-o3","offset");
   ExpectError("catalog: absurd offset",pre+"c-o4\",\"cmd\":\"list_symbols\",\"args\":{\"offset\":99999999999}}",-1,"c-o4","offset");
  }

void TestCatalogLive()
  {
   CJson rep;
   int d=-1;

   //--- get_broker: company, server and trade mode only
   string tmode="";
   if(!Jarvis_TradeMode(tmode))
     {
      ExpectError("catalog: get_broker without account fails cleanly","{\"id\":\"b0\",\"cmd\":\"get_broker\"}",JARVIS_RC_INTERNAL,"b0");
     }
   else
     {
      ExpectOk("catalog: get_broker","{\"id\":\"b1\",\"cmd\":\"get_broker\",\"args\":{}}",rep,true);
      d=rep.Get(rep.Root(),"data");
      Check(rep.Count(d)==3 && rep.Str(rep.Get(d,"company"))==AccountInfoString(ACCOUNT_COMPANY) &&
            rep.Str(rep.Get(d,"server"))==AccountInfoString(ACCOUNT_SERVER) && rep.Str(rep.Get(d,"trade_mode"))==tmode &&
            rep.Get(d,"login")<0 && rep.Get(d,"name")<0,"catalog: get_broker has company, server, trade_mode and no identity");
     }

   //--- list_symbols against the real server catalog
   int serverTotal=SymbolsTotal(false);
   if(serverTotal<=0)
     {
      Print("      no symbols on the server list: live list_symbols checks skipped");
      return;
     }
   int watchBefore=SymbolsTotal(true);
   Jarvis_CatalogInvalidate();
   int buildsBefore=g_jarvisCatBuilds;
   ulong t0=GetTickCount64();

   if(!ListPage("c-p0",0,100,rep,d))
      return;
   ulong firstMs=GetTickCount64()-t0;
   int total=(int)rep.Int(rep.Get(d,"total"));
   int shown=rep.Count(rep.Get(d,"symbols"));
   Check(total==serverTotal && shown==MathMin(100,total) && rep.Int(rep.Get(d,"offset"))==0,
         "catalog: first page: total matches the server list",StringFormat("total=%d server=%d shown=%d",total,serverTotal,shown));
   PrintFormat("      server catalog: %d symbols, first page (includes building the sorted list) took %I64u ms",total,firstMs);

   //--- entries: eight fields each, ascending names, known trade modes
   int syms=rep.Get(d,"symbols");
   bool shapeOk=true,orderOk=true;
   for(int i=0; i<rep.Count(syms); i++)
     {
      int e=rep.At(syms,i);
      string mode=rep.Str(rep.Get(e,"trade_mode"));
      if(rep.Count(e)!=8 || rep.Str(rep.Get(e,"name"))=="" || rep.Type(rep.Get(e,"description"))!=JV_STRING ||
         rep.Type(rep.Get(e,"path"))!=JV_STRING || rep.Type(rep.Get(e,"visible"))!=JV_BOOL || !rep.IsWhole(rep.Get(e,"digits")) ||
         rep.Type(rep.Get(e,"currency_base"))!=JV_STRING || rep.Type(rep.Get(e,"currency_profit"))!=JV_STRING ||
         (mode!="full" && mode!="long_only" && mode!="short_only" && mode!="close_only" && mode!="disabled"))
         shapeOk=false;
      if(i>0 && StringCompare(rep.Str(rep.Get(rep.At(syms,i-1),"name")),rep.Str(rep.Get(e,"name")),true)>0)
         orderOk=false;
     }
   Check(shapeOk,"catalog: every entry has the eight fields and a known trade_mode");
   Check(orderOk,"catalog: entries are in ascending name order");

   //--- paging: two small pages equal one page of the same span; the last page is partial
   if(total>=14)
     {
      CJson p1,p2,p3;
      int d1,d2,d3;
      if(ListPage("c-p1",0,7,p1,d1) && ListPage("c-p2",7,7,p2,d2) && ListPage("c-p3",0,14,p3,d3))
        {
         int s1=p1.Get(d1,"symbols"),s2=p2.Get(d2,"symbols"),s3=p3.Get(d3,"symbols");
         bool same=(p1.Count(s1)==7 && p2.Count(s2)==7 && p3.Count(s3)==14);
         for(int i=0; i<14 && same; i++)
           {
            string a=(i<7)?p1.Str(p1.Get(p1.At(s1,i),"name")):p2.Str(p2.Get(p2.At(s2,i-7),"name"));
            string b=p3.Str(p3.Get(p3.At(s3,i),"name"));
            if(a!=b)
               same=false;
           }
         Check(same && p2.Int(p2.Get(d2,"offset"))==7,"catalog: two pages of 7 equal one page of 14");
        }
     }
   CJson last,end,beyond;
   int dl,de,db;
   if(ListPage("c-p4",total-1,5,last,dl))
      Check(last.Count(last.Get(dl,"symbols"))==1 && last.Int(last.Get(dl,"total"))==total,"catalog: last page holds the remaining symbol");
   if(ListPage("c-p5",total,5,end,de))
      Check(end.Count(end.Get(de,"symbols"))==0 && end.Int(end.Get(de,"total"))==total && end.Int(end.Get(de,"offset"))==total,
            "catalog: offset == total gives an empty page");
   if(ListPage("c-p6",total+5000,5,beyond,db))
      Check(beyond.Count(beyond.Get(db,"symbols"))==0 && beyond.Int(beyond.Get(db,"total"))==total && beyond.Int(beyond.Get(db,"offset"))==total+5000,
            "catalog: offset past the end gives an empty page, not an error");

   //--- default page (500) and the largest page (1000): checked on the text, as the test parser caps nodes
   JarvisResult r;
   Jarvis_Process("{\"id\":\"c-d\",\"cmd\":\"list_symbols\"}",r,false);
   int nDefault=CountOccurrences(r.reply,"\"name\":\"");
   Check(r.ok && nDefault==MathMin(JARVIS_LIST_SYMBOLS_DEFAULT,total) && StringFind(r.reply,"\"offset\":0")>0,
         "catalog: default page is 500 symbols",StringFormat("n=%d ok=%s",nDefault,r.ok?"true":"false"));
   PrintFormat("      default page: %d symbols, reply %d characters",nDefault,StringLen(r.reply));
   ulong t1=GetTickCount64();
   Jarvis_Process("{\"id\":\"c-m\",\"cmd\":\"list_symbols\",\"args\":{\"limit\":1000}}",r,false);
   ulong pageMs=GetTickCount64()-t1;
   int nMax=CountOccurrences(r.reply,"\"name\":\"");
   Check(r.ok && nMax==MathMin(JARVIS_LIST_SYMBOLS_MAX,total),"catalog: largest page is 1000 symbols",StringFormat("n=%d",nMax));
   uchar bytes[];
   Jarvis_Utf8(r.reply,bytes);
   PrintFormat("      page of %d symbols: %d bytes on the wire, %I64u ms (sorted list cached)",nMax,ArraySize(bytes),pageMs);
   Check(ArraySize(bytes)<1500000,"catalog: a full page stays well under 1.5 MB");

   //--- the sorted list was built once for all of the above, and Market Watch is untouched
   Check(g_jarvisCatBuilds-buildsBefore<=2,"catalog: the sorted list is cached between requests",
         StringFormat("%d builds",g_jarvisCatBuilds-buildsBefore));
   Check(SymbolsTotal(true)==watchBefore,"catalog: Market Watch unchanged by list_symbols",
         StringFormat("before %d, after %d",watchBefore,SymbolsTotal(true)));
  }

void TestCatalog()
  {
   TestCatalogSort();
   TestCatalogEscaping();
   TestCatalogArguments();
   TestCatalogLive();
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

//--- the per-order cap g_jarvisMaxLot (EA input InpMaxLot). The cap is restored afterwards.
void TestMaxLot()
  {
   //--- every request here names a symbol that does not exist, so even if the cap were broken the
   //--- request would still be rejected (unknown symbol or the safety gate) and nothing could trade
   const double saved=g_jarvisMaxLot;
   g_jarvisMaxLot=1.0;
   ExpectError("maxlot: order above the cap is refused",BuildOrder("ml1","NOPE_X","buy","market",1.01,-1,-1,-1),-1,"ml1","volume exceeds the EA's InpMaxLot (1)");
   ExpectError("maxlot: far above the cap is refused",BuildOrder("ml2","NOPE_X","sell","market",50,-1,-1,-1),-1,"ml2","InpMaxLot");
   ExpectError("maxlot: a pending order above the cap is refused",BuildOrder("ml3","NOPE_X","buy","limit",2.5,1.0,-1,-1),-1,"ml3","InpMaxLot");
   //--- exactly at the cap is allowed past the cap check: whatever answers next must not be the cap
   JarvisResult r;
   CJson rep;
   if(Run(BuildOrder("ml4","NOPE_X","buy","market",1.0,-1,-1,-1),r,rep))
      Check(StringFind(rep.Str(rep.Get(rep.Root(),"error")),"InpMaxLot")<0,"maxlot: an order exactly at the cap passes the cap check",r.reply);
   g_jarvisMaxLot=0.5;
   ExpectError("maxlot: a lower cap applies",BuildOrder("ml5","NOPE_X","buy","market",0.6,-1,-1,-1),-1,"ml5","InpMaxLot (0.5)");
   //--- 0 disables the cap
   g_jarvisMaxLot=0.0;
   if(Run(BuildOrder("ml6","NOPE_X","buy","market",50,-1,-1,-1),r,rep))
      Check(StringFind(rep.Str(rep.Get(rep.Root(),"error")),"InpMaxLot")<0,"maxlot: 0 disables the cap",r.reply);
   //--- closes are never capped: a partial close of 5 lots on a ticket that cannot exist fails on the ticket, not on the cap
   g_jarvisMaxLot=1.0;
   if(Run("{\"id\":\"ml7\",\"cmd\":\"position_close_partial\",\"args\":{\"ticket\":9000000000000000,\"volume\":5}}",r,rep))
      Check(StringFind(rep.Str(rep.Get(rep.Root(),"error")),"InpMaxLot")<0,"maxlot: position_close_partial is not capped",r.reply);
   g_jarvisMaxLot=saved;
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

   //--- InpMaxLot cap (EA-1): answered with -1 before the safety gate and before any market lookup,
   //--- so these run on every account and trade nothing
   TestMaxLot();

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
   Jarvis_Process(firstReq,r1,false);
   ulong hits=g_jarvisCacheHits;
   string otherReq="{\"id\":\"dup-1\",\"cmd\":\"position_close\",\"args\":{\"ticket\":999999999999}}";   // other command, same id
   Jarvis_Process(otherReq,r2,false);
   Check(!r1.ok && !r1.cached && r2.reply==r1.reply && r2.cached && g_jarvisCacheHits==hits+1,
         "write cache: a repeated id returns the cached reply, nothing runs again",r2.reply);
   Jarvis_Process(firstReq,r3,false);
   Check(r3.reply==r1.reply && r3.cached && g_jarvisCacheHits==hits+2,"write cache: still cached on the third request");
   //--- the ring holds 64 replies: after 64 other ids the first one is forgotten
   for(int i=0; i<JARVIS_CACHE_SIZE; i++)
     {
      JarvisResult rx;
      Jarvis_Process(StringFormat("{\"id\":\"ring-%d\",\"cmd\":\"order_cancel\",\"args\":{\"ticket\":999999999999}}",i),rx,false);
     }
   Jarvis_Process(otherReq,r2,false);
   Check(!r2.cached && r2.reply!=r1.reply,"write cache: the oldest reply is evicted after 64 newer ones");
   ulong h2=g_jarvisCacheHits;
   JarvisResult r4;
   Jarvis_Process("{\"id\":\"ring-63\",\"cmd\":\"order_cancel\",\"args\":{\"ticket\":999999999999}}",r4,false);
   Check(r4.cached && g_jarvisCacheHits==h2+1,"write cache: a recent id is still cached");

   Check(PositionsTotal()==posBefore && OrdersTotal()==ordBefore,"write: no position or order was created or changed by the rejected requests");
  }

//+------------------------------------------------------------------+
//| Authentication (BR-1): HMAC-SHA256 against the RFC 4231 vectors, |
//| the golden vector shared with the Rust tests, and the request    |
//| pipeline Jarvis_HandleSigned() with wrong keys, tampering,       |
//| replays and stale timestamps. Nothing is traded.                 |
//+------------------------------------------------------------------+
//--- golden vector: crates/bridge-client/tests/fixtures/auth.golden.json (checked by a Rust test)
#define GOLDEN_KEY            "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f"
#define GOLDEN_REQUEST        "{\"id\":\"6f1c2c1e-3b5a-4d0e-9a53-0d2a6f8d1001\",\"ts\":1793341200000,\"cmd\":\"ping\",\"args\":{}}"
#define GOLDEN_REQUEST_MAC    "bc45b3fc8bc3cef210abe4efff32897c8ca52cf7a0da1d0366707923cd1ca581"
#define GOLDEN_REPLY          "{\"id\":\"6f1c2c1e-3b5a-4d0e-9a53-0d2a6f8d1001\",\"ok\":true,\"data\":{\"protocol_version\":2,\"ea_version\":\"0.3.0\",\"trade_mode\":\"demo\",\"terminal_connected\":true,\"trade_allowed\":true,\"server_time\":1793341200},\"retcode\":0,\"ts\":1793341200012}"
#define GOLDEN_REPLY_MAC      "63b14665d262978eb9983b937d78ed251299d1ea085472b1b66628e94717c80b"
#define OTHER_KEY             "a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5"
#define NOW_MS                1793341200000

void FillBytes(uchar &b[],const int n,const uchar v)
  {
   ArrayResize(b,n);
   for(int i=0; i<n; i++)
      b[i]=v;
  }

void HmacCase(const string name,const uchar &key[],const uchar &data[],const string want)
  {
   string got=Jarvis_HmacHex(key,data);
   Check(got==want,name,got);
  }

void TestHmac()
  {
   uchar k[],d[];
   //--- SHA-256("abc")
   uchar abc[],h[];
   Jarvis_Utf8("abc",abc);
   Check(Jarvis_Sha256(abc,h) && Jarvis_HexEncode(h)=="ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad","sha256: abc");

   //--- RFC 4231 section 4 (case 5 is a truncated output and is not applicable)
   FillBytes(k,20,0x0b);
   Jarvis_Utf8("Hi There",d);
   HmacCase("hmac: RFC 4231 case 1",k,d,"b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7");

   Jarvis_Utf8("Jefe",k);
   Jarvis_Utf8("what do ya want for nothing?",d);
   HmacCase("hmac: RFC 4231 case 2",k,d,"5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");

   FillBytes(k,20,0xaa);
   FillBytes(d,50,0xdd);
   HmacCase("hmac: RFC 4231 case 3",k,d,"773ea91e36800e46854db8ebd09181a72959098b3ef8c122d9635514ced565fe");

   ArrayResize(k,25);
   for(int i=0; i<25; i++)
      k[i]=(uchar)(i+1);
   FillBytes(d,50,0xcd);
   HmacCase("hmac: RFC 4231 case 4",k,d,"82558a389a443c0ea4cc819899f2083a85f0faa3e578f8077a2e3ff46729665b");

   FillBytes(k,131,0xaa);
   Jarvis_Utf8("Test Using Larger Than Block-Size Key - Hash Key First",d);
   HmacCase("hmac: RFC 4231 case 6 (key longer than the block)",k,d,"60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54");

   Jarvis_Utf8("This is a test using a larger than block-size key and a larger than block-size data. "+
               "The key needs to be hashed before being used by the HMAC algorithm.",d);
   HmacCase("hmac: RFC 4231 case 7 (key and data longer than the block)",k,d,"9b09ffa71b942fcb27635fbcd5b0e944bfdc63644f0713938a7f51535c3a35e2");

   //--- an empty message and a 64-byte key (exactly one block) must work too
   uchar none[];
   FillBytes(k,64,0x11);
   Check(StringLen(Jarvis_HmacHex(k,none))==64,"hmac: empty message gives a 32-byte MAC");
  }

void TestEncoding()
  {
   uchar b[];
   Check(Jarvis_HexDecode("00ff10Ab",b) && ArraySize(b)==4 && b[0]==0 && b[1]==255 && b[2]==16 && b[3]==171,"hex: decode, either case");
   Check(Jarvis_HexEncode(b)=="00ff10ab","hex: encode is lowercase");
   uchar t[];
   Check(!Jarvis_HexDecode("abc",t) && !Jarvis_HexDecode("zz",t) && !Jarvis_HexDecode("0g",t),"hex: odd length and non-hex rejected");

   uchar a1[],a2[],a3[],a4[];
   Jarvis_AsciiBytes("abcd",a1);
   Jarvis_AsciiBytes("abcd",a2);
   Jarvis_AsciiBytes("abce",a3);
   Jarvis_AsciiBytes("abc",a4);
   Check(Jarvis_ConstTimeEqual(a1,a2) && !Jarvis_ConstTimeEqual(a1,a3) && !Jarvis_ConstTimeEqual(a1,a4),"ct-compare: equal, different, different length");
   uchar e1[],e2[];
   Check(Jarvis_ConstTimeEqual(e1,e2),"ct-compare: two empty arrays are equal");

   //--- UTF-8: e acute (2 bytes) and euro sign (3 bytes), no terminating zero
   uchar u[];
   Jarvis_Utf8(ShortToString(0xE9)+ShortToString(0x20AC),u);
   Check(ArraySize(u)==5 && u[0]==0xC3 && u[1]==0xA9 && u[2]==0xE2 && u[3]==0x82 && u[4]==0xAC,"utf8: multi-byte characters, no terminator",IntegerToString(ArraySize(u)));
   Jarvis_Utf8("",u);
   Check(ArraySize(u)==0,"utf8: empty string gives no bytes");
   Jarvis_Utf8("ab",u);
   Check(ArraySize(u)==2 && u[0]=='a' && u[1]=='b',"utf8: ascii, no terminator");

   //--- key validation
   string err="";
   Check(!Jarvis_SetKeyHex("",err) && !g_jarvisKeyReady,"key: empty is refused");
   Check(!Jarvis_SetKeyHex("abcd",err),"key: too short is refused");
   Check(!Jarvis_SetKeyHex(StringSubstr(GOLDEN_KEY,0,62)+"zz",err),"key: non-hex is refused");
   Check(!Jarvis_SetKeyHex("0000000000000000000000000000000000000000000000000000000000000000",err),"key: all zeros is refused");
   Check(Jarvis_SetKeyHex("  "+GOLDEN_KEY+"  ",err) && g_jarvisKeyReady && ArraySize(g_jarvisKey)==32,"key: blanks are ignored");
   string upper=GOLDEN_KEY;
   StringToUpper(upper);
   Check(Jarvis_SetKeyHex(upper,err) && g_jarvisKey[31]==0x1f,"key: uppercase hex accepted");
  }

void TestGolden()
  {
   string err="";
   Check(Jarvis_SetKeyHex(GOLDEN_KEY,err),"golden: key loads",err);
   uchar body[],frame[];
   Jarvis_Utf8(GOLDEN_REQUEST,body);
   Check(Jarvis_ExpectedMacFrame(body,frame) && CharArrayToString(frame,0,WHOLE_ARRAY,CP_ACP)==GOLDEN_REQUEST_MAC,
         "golden: request MAC equals the Rust value",CharArrayToString(frame,0,WHOLE_ARRAY,CP_ACP));
   uchar mac[];
   Jarvis_AsciiBytes(GOLDEN_REQUEST_MAC,mac);
   Check(Jarvis_VerifyFrames(mac,body),"golden: request frames verify");
   Jarvis_Utf8(GOLDEN_REPLY,body);
   Jarvis_AsciiBytes(GOLDEN_REPLY_MAC,mac);
   Check(Jarvis_VerifyFrames(mac,body),"golden: reply frames verify");
   Jarvis_AsciiBytes(StringSubstr(GOLDEN_REPLY_MAC,0,63)+"0",mac);
   Check(!Jarvis_VerifyFrames(mac,body),"golden: one wrong digit fails");
   string up=GOLDEN_REPLY_MAC;
   StringToUpper(up);
   Jarvis_AsciiBytes(up,mac);
   Check(!Jarvis_VerifyFrames(mac,body),"golden: uppercase MAC is refused (frames are lowercase hex)");
  }

//--- the two frames of a message signed with keyHex
void SignedFrames(const string keyHex,const string payload,uchar &mac[],uchar &body[])
  {
   uchar k[];
   Jarvis_HexDecode(keyHex,k);
   Jarvis_Utf8(payload,body);
   Jarvis_AsciiBytes(Jarvis_HmacHex(k,body),mac);
  }

string ReqJson(const string id,const long ts,const string cmd="launch_missiles")
  {
   return StringFormat("{\"id\":\"%s\",\"ts\":%I64d,\"cmd\":\"%s\",\"args\":{}}",id,ts,cmd);
  }

//--- the reply must be the generic refusal: -4, "authentication failed", no data, the expected id
void ExpectRefused(const string name,const int frames,const uchar &mac[],const uchar &body[],const string expectId)
  {
   JarvisResult r;
   CJson rep;
   Jarvis_HandleSigned(frames,mac,body,r);
   bool ok=rep.Parse(r.reply);
   int root=rep.Root();
   ok=ok && rep.IsObject(root) && !rep.Bool(rep.Get(root,"ok")) && rep.Int(rep.Get(root,"retcode"))==-4 &&
      rep.Str(rep.Get(root,"error"))=="authentication failed" && rep.Str(rep.Get(root,"id"))==expectId &&
      rep.Get(root,"data")<0 && rep.IsWhole(rep.Get(root,"ts")) && !r.ok && r.retcode==-4;
   Check(ok,name,r.reply);
  }

void TestPipeline()
  {
   string err="";
   Jarvis_SetKeyHex(GOLDEN_KEY,err);
   g_jarvisClockOverrideMs=NOW_MS;
   g_jarvisMonoOverrideOn=true;
   g_jarvisMonoOverrideMs=1000000;
   Jarvis_ReplayReset();
   ulong refusedBefore=g_jarvisAuthRefused;

   uchar mac[],body[],none[];
   JarvisResult r;
   CJson rep;

   //--- a genuine request passes authentication and reaches the dispatcher (unknown command: -2)
   SignedFrames(GOLDEN_KEY,ReqJson("a-1",NOW_MS),mac,body);
   Jarvis_HandleSigned(2,mac,body,r);
   Check(!r.ok && r.retcode==-2 && r.id=="a-1","auth: a signed, fresh request reaches the dispatcher",r.reply);
   Check(rep.Parse(r.reply) && rep.IsWhole(rep.Get(rep.Root(),"ts")) &&
         MathAbs((double)rep.Int(rep.Get(rep.Root(),"ts"))-(double)NOW_MS)<=1.0,"auth: replies carry ts");
   //--- the reply is signed over its exact bytes
   uchar rbytes[],rframe[];
   Jarvis_Utf8(r.reply,rbytes);
   Check(Jarvis_ExpectedMacFrame(rbytes,rframe) && ArraySize(rframe)==64,"auth: a reply can be signed (64 hex digits)");

   //--- replay: the very same message again
   ExpectRefused("auth: a replayed id is refused (the id is echoed to a key holder)",2,mac,body,"a-1");
   //--- ... and so is the same id with a new timestamp
   SignedFrames(GOLDEN_KEY,ReqJson("a-1",NOW_MS+5),mac,body);
   ExpectRefused("auth: the same id with a new ts is still a replay",2,mac,body,"a-1");

   //--- wrong key: refused, and the id is NOT echoed (no signing oracle)
   SignedFrames(OTHER_KEY,ReqJson("b-1",NOW_MS),mac,body);
   ExpectRefused("auth: a request signed with another key is refused",2,mac,body,"");
   //--- missing / malformed frames
   ExpectRefused("auth: a single frame is refused",1,none,body,"");
   uchar plain[];
   Jarvis_Utf8(ReqJson("b-2",NOW_MS),plain);
   ExpectRefused("auth: an unsigned request (payload as the only frame) is refused",1,plain,none,"");
   SignedFrames(GOLDEN_KEY,ReqJson("b-3",NOW_MS),mac,body);
   ExpectRefused("auth: three frames are refused even with a valid pair",3,mac,body,"");
   ExpectRefused("auth: zero frames are refused",0,none,none,"");
   ExpectRefused("auth: an empty MAC frame is refused",2,none,body,"");
   uchar upMac[];
   string upText=CharArrayToString(mac,0,WHOLE_ARRAY,CP_ACP);
   StringToUpper(upText);
   Jarvis_AsciiBytes(upText,upMac);
   ExpectRefused("auth: an uppercase MAC is refused",2,upMac,body,"");
   //--- tampering after signing
   uchar bad[];
   ArrayCopy(bad,body);
   bad[10]=(uchar)(bad[10]^0x01);
   ExpectRefused("auth: a payload changed after signing is refused",2,mac,bad,"");
   uchar badMac[];
   ArrayCopy(badMac,mac);
   badMac[0]=(badMac[0]=='0')?'1':'0';
   ExpectRefused("auth: a MAC with one digit changed is refused",2,badMac,body,"");
   //--- the refused ids were not consumed
   Jarvis_HandleSigned(2,mac,body,r);
   Check(!r.ok && r.retcode==-2 && r.id=="b-3","auth: refusals do not consume the request id",r.reply);

   //--- timestamps: +-30 s is fine, beyond is stale; missing or non-integer is refused
   SignedFrames(GOLDEN_KEY,ReqJson("c-1",NOW_MS-30000),mac,body);
   Jarvis_HandleSigned(2,mac,body,r);
   Check(r.retcode==-2,"auth: ts exactly 30 s old is accepted",r.reply);
   SignedFrames(GOLDEN_KEY,ReqJson("c-2",NOW_MS+30000),mac,body);
   Jarvis_HandleSigned(2,mac,body,r);
   Check(r.retcode==-2,"auth: ts exactly 30 s ahead is accepted",r.reply);
   SignedFrames(GOLDEN_KEY,ReqJson("c-3",NOW_MS-30001),mac,body);
   ExpectRefused("auth: ts 30.001 s old is stale",2,mac,body,"c-3");
   SignedFrames(GOLDEN_KEY,ReqJson("c-4",NOW_MS+30001),mac,body);
   ExpectRefused("auth: ts 30.001 s ahead is refused",2,mac,body,"c-4");
   SignedFrames(GOLDEN_KEY,ReqJson("c-5",0),mac,body);
   ExpectRefused("auth: ts 0 is stale",2,mac,body,"c-5");
   SignedFrames(GOLDEN_KEY,ReqJson("c-6",-5),mac,body);
   ExpectRefused("auth: negative ts is refused",2,mac,body,"c-6");
   SignedFrames(GOLDEN_KEY,"{\"id\":\"c-7\",\"cmd\":\"launch_missiles\"}",mac,body);
   ExpectRefused("auth: a missing ts is refused",2,mac,body,"c-7");
   SignedFrames(GOLDEN_KEY,"{\"id\":\"c-8\",\"ts\":\"now\",\"cmd\":\"launch_missiles\"}",mac,body);
   ExpectRefused("auth: a ts that is a string is refused",2,mac,body,"c-8");
   SignedFrames(GOLDEN_KEY,"{\"id\":\"c-9\",\"ts\":1.5,\"cmd\":\"launch_missiles\"}",mac,body);
   ExpectRefused("auth: a fractional ts is refused",2,mac,body,"c-9");
   SignedFrames(GOLDEN_KEY,"{\"id\":\"c-10\",\"ts\":1e300,\"cmd\":\"launch_missiles\"}",mac,body);
   ExpectRefused("auth: an absurd ts is refused",2,mac,body,"c-10");

   //--- authentic but not JSON: the usual -1, nothing else
   SignedFrames(GOLDEN_KEY,"hello",mac,body);
   Jarvis_HandleSigned(2,mac,body,r);
   Check(!r.ok && r.retcode==-1,"auth: authentic garbage is an ordinary -1",r.reply);
   uchar zero[];
   Jarvis_Utf8("{\"id\":\"z\"}",zero);
   ArrayResize(zero,ArraySize(zero)+1);
   zero[ArraySize(zero)-1]=0;
   uchar zmac[];
   Jarvis_AsciiBytes(Jarvis_HmacHex(g_jarvisKey,zero),zmac);
   Jarvis_HandleSigned(2,zmac,zero,r);
   Check(!r.ok && r.retcode==-1 && StringFind(r.reply,"zero byte")>=0,"auth: a payload with a zero byte is -1",r.reply);

   //--- the id window: after 60 s the id may be used again (its old ts is long stale, a new one is fresh)
   SignedFrames(GOLDEN_KEY,ReqJson("d-1",NOW_MS),mac,body);
   Jarvis_HandleSigned(2,mac,body,r);
   Check(r.retcode==-2,"auth: id d-1 accepted once");
   g_jarvisMonoOverrideMs+=JARVIS_REPLAY_WINDOW_MS;
   ExpectRefused("auth: still remembered after exactly 60 s",2,mac,body,"d-1");
   g_jarvisMonoOverrideMs+=1;
   Jarvis_HandleSigned(2,mac,body,r);
   Check(r.retcode==-2,"auth: forgotten 60 s after it was used",r.reply);

   //--- capacity: 4096 ids in the window, the next request is refused until some expire
   Jarvis_ReplayReset();
   bool allOk=true;
   for(int i=0; i<JARVIS_REPLAY_CAPACITY; i++)
      if(Jarvis_FreshnessCheck("cap-"+IntegerToString(i),NOW_MS)!=JARVIS_FRESH_OK)
         allOk=false;
   Check(allOk && g_jarvisSeenCount==JARVIS_REPLAY_CAPACITY,"replay table: holds 4096 ids");
   Check(Jarvis_FreshnessCheck("cap-extra",NOW_MS)==JARVIS_FRESH_FULL,"replay table: full means refuse (fail closed)");
   g_jarvisMonoOverrideMs+=JARVIS_REPLAY_WINDOW_MS+1;
   Check(Jarvis_FreshnessCheck("cap-extra",NOW_MS)==JARVIS_FRESH_OK && g_jarvisSeenCount==1,"replay table: recovers when the ids expire");

   //--- no key loaded: everything is refused
   g_jarvisKeyReady=false;
   SignedFrames(GOLDEN_KEY,ReqJson("e-1",NOW_MS),mac,body);
   ExpectRefused("auth: without a key everything is refused",2,mac,body,"");
   Jarvis_SetKeyHex(GOLDEN_KEY,err);

   //--- nothing executes on a refused request: a write with bad arguments would be answered
   //--- with -1 and cached by the write path; refused, it is neither
   string badOrder="{\"id\":\"w-1\",\"ts\":"+IntegerToString(NOW_MS)+",\"cmd\":\"order_send\",\"args\":{\"symbol\":\"EURUSD\",\"side\":\"hold\",\"volume\":0,\"type\":\"market\",\"magic\":1,\"comment\":\"x\"}}";
   int cacheBefore=g_jarvisCacheCount;
   SignedFrames(OTHER_KEY,badOrder,mac,body);
   ExpectRefused("auth: a write signed with another key is refused",2,mac,body,"");
   Check(g_jarvisCacheCount==cacheBefore,"auth: the refused write never reached the write path");
   SignedFrames(GOLDEN_KEY,badOrder,mac,body);
   Jarvis_HandleSigned(2,mac,body,r);
   Check(!r.ok && r.retcode==-1 && g_jarvisCacheCount==cacheBefore+1,"auth: the same write with the right key reaches validation (-1) and the cache",r.reply);
   ExpectRefused("auth: replaying that write is refused before the cache is consulted",2,mac,body,"w-1");
   Check(g_jarvisCacheCount==cacheBefore+1,"auth: the replay changed nothing");

   Check(g_jarvisAuthRefused>refusedBefore,"auth: refusals are counted");

   //--- real clock sanity: milliseconds track TimeGMT() and never run backwards within a second
   g_jarvisClockOverrideMs=0;
   g_jarvisMonoOverrideOn=false;
   long n1=Jarvis_NowMs();
   long n2=Jarvis_NowMs();
   long gmtMs=(long)TimeGMT()*1000;
   Check(n2>=n1 && n1>=gmtMs-1000 && n1<=gmtMs+1999,"clock: Jarvis_NowMs() follows TimeGMT()",StringFormat("%I64d vs %I64d",n1,gmtMs));

   Jarvis_ReplayReset();
   ArrayInitialize(g_jarvisKey,0);
   g_jarvisKeyReady=false;
  }

void TestAuth()
  {
   TestHmac();
   TestEncoding();
   TestGolden();
   TestPipeline();
  }

void OnStart()
  {
   Print("=== JarvisSelfTest start ===");
   TestAuth();
   TestJson();
   TestProtocol();
   TestLive();
   TestCatalog();
   TestWrites();
   PrintFormat("=== JarvisSelfTest done: %d passed, %d failed ===",g_pass,g_fail);
  }
