//+------------------------------------------------------------------+
//| SymbolCatalog.mqh - get_broker and list_symbols.                 |
//|                                                                  |
//| get_broker  {}               -> {company, server, trade_mode}    |
//| list_symbols {offset?,limit?} -> {symbols[], total, offset}      |
//|                                                                  |
//| list_symbols pages through ALL symbols the trade server offers   |
//| (SymbolsTotal(false) / SymbolName(i,false)), not just Market     |
//| Watch, ordered by name. Nothing here selects a symbol: Market    |
//| Watch is never changed. Brokers have up to ~12,000 symbols, so:  |
//|   - the sorted name list is built once and cached for 30 s (it is|
//|     rebuilt early when the symbol count, the account or the      |
//|     server changes). Building = one pass over SymbolName() and a |
//|     merge sort of an index array: no ArraySort on strings, whose |
//|     behaviour MetaQuotes does not specify.                       |
//|   - only the requested page (default 500, max 1000 symbols) is   |
//|     read from the terminal and serialised.                       |
//| Handlers only read terminal state. They never trade.             |
//| Wire format: crates/bridge-client/src/protocol.rs                |
//+------------------------------------------------------------------+
#ifndef JARVIS_SYMBOL_CATALOG_MQH
#define JARVIS_SYMBOL_CATALOG_MQH

#include "Auth.mqh"
#include "Json.mqh"
#include "ReadHandlers.mqh"

#define JARVIS_LIST_SYMBOLS_DEFAULT   500
#define JARVIS_LIST_SYMBOLS_MAX       1000
#define JARVIS_LIST_SYMBOLS_OFFSET_MAX 10000000
#define JARVIS_CATALOG_TTL_MS         30000

//--- cached, sorted symbol names of the whole server catalog
string g_jarvisCatNames[];
int    g_jarvisCatCount=0;
bool   g_jarvisCatValid=false;
int    g_jarvisCatServerTotal=-1;   // SymbolsTotal(false) when the list was built
long   g_jarvisCatLogin=0;          // account the list belongs to (never sent)
string g_jarvisCatServer="";
ulong  g_jarvisCatBuiltMs=0;        // Jarvis_MonoMs() when the list was built
int    g_jarvisCatBuilds=0;         // how many times it was built (self-test)

void Jarvis_CatalogInvalidate()
  {
   g_jarvisCatValid=false;
   g_jarvisCatCount=0;
   ArrayFree(g_jarvisCatNames);
  }

//+------------------------------------------------------------------+
//| Sort names[0..n-1] ascending, case-sensitive (StringCompare), in |
//| place. Bottom-up merge sort over an index array with two         |
//| ping-pong buffers: O(n log n) comparisons, no string copies until|
//| the final permutation. Stable. false only if memory ran out (the |
//| array is then left unchanged).                                   |
//+------------------------------------------------------------------+
bool Jarvis_SortNames(string &names[],const int n)
  {
   if(n<2)
      return true;
   int src[],dst[];
   if(ArrayResize(src,n)!=n || ArrayResize(dst,n)!=n)
      return false;
   for(int i=0; i<n; i++)
      src[i]=i;
   for(int width=1; width<n; width*=2)
     {
      for(int lo=0; lo<n; lo+=2*width)
        {
         int mid=MathMin(lo+width,n);
         int hi=MathMin(lo+2*width,n);
         int a=lo,b=mid,k=lo;
         while(a<mid && b<hi)
           {
            if(StringCompare(names[src[b]],names[src[a]],true)<0)
               dst[k++]=src[b++];
            else
               dst[k++]=src[a++];
           }
         while(a<mid)
            dst[k++]=src[a++];
         while(b<hi)
            dst[k++]=src[b++];
        }
      ArrayCopy(src,dst,0,0,n);
     }
   string sorted[];
   if(ArrayResize(sorted,n)!=n)
      return false;
   for(int i=0; i<n; i++)
      sorted[i]=names[src[i]];
   ArrayCopy(names,sorted,0,0,n);
   return true;
  }

//--- true when names[0..n-1] is ascending (StringCompare, case-sensitive)
bool Jarvis_NamesSorted(const string &names[],const int n)
  {
   for(int i=1; i<n; i++)
      if(StringCompare(names[i-1],names[i],true)>0)
         return false;
   return true;
  }

//+------------------------------------------------------------------+
//| Make sure the cached sorted list is current. Returns the number  |
//| of symbols (>=0), or -1 with err set.                            |
//+------------------------------------------------------------------+
int Jarvis_CatalogEnsure(string &err)
  {
   ResetLastError();
   int serverTotal=SymbolsTotal(false);
   long login=AccountInfoInteger(ACCOUNT_LOGIN);
   string server=AccountInfoString(ACCOUNT_SERVER);
   ulong now=Jarvis_MonoMs();
   if(g_jarvisCatValid && serverTotal==g_jarvisCatServerTotal && login==g_jarvisCatLogin &&
      server==g_jarvisCatServer && (long)(now-g_jarvisCatBuiltMs)<JARVIS_CATALOG_TTL_MS)
      return g_jarvisCatCount;

   Jarvis_CatalogInvalidate();
   if(serverTotal<0)
     {
      err="symbol list is not available";
      return -1;
     }
   string names[];
   if(serverTotal>0 && ArrayResize(names,serverTotal)!=serverTotal)
     {
      err="out of memory while reading the symbol list";
      return -1;
     }
   int n=0;
   for(int i=0; i<serverTotal; i++)
     {
      string name=SymbolName(i,false);
      if(name!="")   // a symbol can vanish between SymbolsTotal() and SymbolName()
         names[n++]=name;
     }
   if(!Jarvis_SortNames(names,n))
     {
      err="out of memory while sorting the symbol list";
      return -1;
     }
   if(n>0)
     {
      if(ArrayResize(g_jarvisCatNames,n)!=n)
        {
         err="out of memory while caching the symbol list";
         return -1;
        }
      ArrayCopy(g_jarvisCatNames,names,0,0,n);
     }
   g_jarvisCatCount=n;
   g_jarvisCatServerTotal=serverTotal;
   g_jarvisCatLogin=login;
   g_jarvisCatServer=server;
   g_jarvisCatBuiltMs=now;
   g_jarvisCatValid=true;
   g_jarvisCatBuilds++;
   return n;
  }

//--- SYMBOL_TRADE_MODE_* -> wire name. Unknown values count as "disabled" (fail safe).
string Jarvis_SymbolTradeModeName(const long m)
  {
   switch((int)m)
     {
      case SYMBOL_TRADE_MODE_FULL:
         return "full";
      case SYMBOL_TRADE_MODE_LONGONLY:
         return "long_only";
      case SYMBOL_TRADE_MODE_SHORTONLY:
         return "short_only";
      case SYMBOL_TRADE_MODE_CLOSEONLY:
         return "close_only";
      case SYMBOL_TRADE_MODE_DISABLED:
         return "disabled";
     }
   return "disabled";
  }

//--- one catalog entry from plain values (also used by the self-test with made-up values)
void Jarvis_PutCatalogEntry(CJsonWriter &w,const string name,const string description,const string path,
                            const string tradeMode,const bool visible,const long digits,
                            const string currencyBase,const string currencyProfit)
  {
   w.BeginObject();
   w.PutStr("name",name);
   w.PutStr("description",description);
   w.PutStr("path",path);
   w.PutStr("trade_mode",tradeMode);
   w.PutBool("visible",visible);
   w.PutInt("digits",digits);
   w.PutStr("currency_base",currencyBase);
   w.PutStr("currency_profit",currencyProfit);
   w.EndObject();
  }

//--- one catalog entry read from the terminal. Read-only: never selects the symbol.
void Jarvis_PutCatalogSymbol(CJsonWriter &w,const string name)
  {
   string description="",path="",base="",profit="";
   long mode=SYMBOL_TRADE_MODE_DISABLED,digits=0,visible=0,selected=0;
   if(!SymbolInfoString(name,SYMBOL_DESCRIPTION,description))
      description="";
   if(!SymbolInfoString(name,SYMBOL_PATH,path))
      path="";
   if(!SymbolInfoString(name,SYMBOL_CURRENCY_BASE,base))
      base="";
   if(!SymbolInfoString(name,SYMBOL_CURRENCY_PROFIT,profit))
      profit="";
   if(!SymbolInfoInteger(name,SYMBOL_TRADE_MODE,mode))
      mode=SYMBOL_TRADE_MODE_DISABLED;
   if(!SymbolInfoInteger(name,SYMBOL_DIGITS,digits) || digits<0)
      digits=0;
   // SYMBOL_VISIBLE: shown in Market Watch. Symbols the terminal selects on its own (cross
   // rates for margin calculation) are selected but not visible; those count as not visible.
   // SYMBOL_SELECT is only the fallback when the property cannot be read.
   bool shown=false;
   if(SymbolInfoInteger(name,SYMBOL_VISIBLE,visible))
      shown=(visible!=0);
   else
      if(SymbolInfoInteger(name,SYMBOL_SELECT,selected))
         shown=(selected!=0);
   Jarvis_PutCatalogEntry(w,name,description,path,Jarvis_SymbolTradeModeName(mode),shown,digits,base,profit);
  }

//+------------------------------------------------------------------+
//| get_broker {}                                                    |
//| Which broker/server the terminal is logged into. Never includes  |
//| the login, the account name or any other identity field.         |
//+------------------------------------------------------------------+
int Jarvis_CmdGetBroker(CJson &req,const int args,CJsonWriter &w,string &err)
  {
   string mode;
   if(!Jarvis_TradeMode(mode))
     {
      err="terminal has no connected trading account";
      return JARVIS_RC_INTERNAL;
     }
   w.BeginObject();
   w.PutStr("company",AccountInfoString(ACCOUNT_COMPANY));
   w.PutStr("server",AccountInfoString(ACCOUNT_SERVER));
   w.PutStr("trade_mode",mode);
   w.EndObject();
   return 0;
  }

//+------------------------------------------------------------------+
//| list_symbols {offset?,limit?}                                    |
//| offset: whole number >= 0 (default 0); at or past the end gives  |
//| an empty page, not an error. limit: 1..1000 (default 500).       |
//| Reply: symbols (name order), total (whole catalog), offset.      |
//+------------------------------------------------------------------+
int Jarvis_CmdListSymbols(CJson &req,const int args,CJsonWriter &w,string &err)
  {
   long offset=0,limit=0;
   bool hasOffset,hasLimit;
   if(!Jarvis_ArgInt(req,args,"offset",offset,hasOffset,err))
      return JARVIS_RC_INVALID;
   if(!Jarvis_ArgInt(req,args,"limit",limit,hasLimit,err))
      return JARVIS_RC_INVALID;
   if(!hasOffset)
      offset=0;
   if(offset<0 || offset>JARVIS_LIST_SYMBOLS_OFFSET_MAX)
      return Jarvis_Invalid(err,StringFormat("'offset' must be between 0 and %d",JARVIS_LIST_SYMBOLS_OFFSET_MAX));
   if(!hasLimit)
      limit=JARVIS_LIST_SYMBOLS_DEFAULT;
   if(limit<1 || limit>JARVIS_LIST_SYMBOLS_MAX)
      return Jarvis_Invalid(err,StringFormat("'limit' must be between 1 and %d",JARVIS_LIST_SYMBOLS_MAX));

   int total=Jarvis_CatalogEnsure(err);
   if(total<0)
      return JARVIS_RC_INTERNAL;

   int first=(int)MathMin((double)offset,(double)total);
   int last=(int)MathMin((double)offset+(double)limit,(double)total);

   w.BeginObject();
   w.Key("symbols");
   w.BeginArray();
   for(int i=first; i<last; i++)
      Jarvis_PutCatalogSymbol(w,g_jarvisCatNames[i]);
   w.EndArray();
   w.PutInt("total",total);
   w.PutInt("offset",offset);
   w.EndObject();
   return 0;
  }

#endif // JARVIS_SYMBOL_CATALOG_MQH
