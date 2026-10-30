//+------------------------------------------------------------------+
//| ReadHandlers.mqh - one function per read command.                |
//|                                                                  |
//| Contract for every handler:                                      |
//|   int Jarvis_Cmd*(CJson &req,int args,CJsonWriter &w,string &err)|
//|   - returns 0 on success after writing the "data" value to w     |
//|   - returns a non-zero retcode and sets err on failure; whatever |
//|     was written to w is discarded by the caller                  |
//|   - retcode -1: bad arguments; MT5 failures: GetLastError()      |
//| Handlers only read terminal state. They never trade.             |
//| Wire format: crates/bridge-client/src/protocol.rs                |
//+------------------------------------------------------------------+
#ifndef JARVIS_READ_HANDLERS_MQH
#define JARVIS_READ_HANDLERS_MQH

#include "Json.mqh"

#define JARVIS_PROTOCOL_VERSION   2
#define JARVIS_EA_VERSION         "0.3.0"

#define JARVIS_RC_INVALID         (-1)
#define JARVIS_RC_UNKNOWN_CMD     (-2)
#define JARVIS_RC_INTERNAL        4001   // ERR_INTERNAL_ERROR
#define JARVIS_RC_UNKNOWN_SYMBOL  4301   // ERR_MARKET_UNKNOWN_SYMBOL
#define JARVIS_RC_NO_TICK         4304   // ERR_MARKET_LAST_TIME_UNKNOWN

#define JARVIS_HISTORY_LIMIT_DEFAULT 100
#define JARVIS_HISTORY_LIMIT_MAX     500
#define JARVIS_MAX_SYMBOL_LEN        32
#define JARVIS_MAX_DATETIME          32535215999   // 3000-12-31 23:59:59, MQL5 datetime max

//+------------------------------------------------------------------+
//| Helpers                                                          |
//+------------------------------------------------------------------+
int Jarvis_Invalid(string &err,const string msg)
  {
   err=msg;
   return JARVIS_RC_INVALID;
  }

//--- failure of an MT5 call: retcode is GetLastError() (never 0 on failure)
int Jarvis_MtFail(string &err,const string msg)
  {
   int e=GetLastError();
   if(e==0)
      e=JARVIS_RC_INTERNAL;
   err=msg;
   return e;
  }

//--- optional string argument. null counts as absent. false => err set.
bool Jarvis_ArgString(CJson &j,const int args,const string key,string &val,bool &present,string &err)
  {
   present=false;
   val="";
   int n=j.Get(args,key);
   if(n<0 || j.Type(n)==JV_NULL)
      return true;
   if(j.Type(n)!=JV_STRING)
     {
      err="argument '"+key+"' must be a string";
      return false;
     }
   val=j.Str(n);
   if(val=="")
     {
      err="argument '"+key+"' must not be empty";
      return false;
     }
   present=true;
   return true;
  }

//--- optional whole-number argument
bool Jarvis_ArgInt(CJson &j,const int args,const string key,long &val,bool &present,string &err)
  {
   present=false;
   val=0;
   int n=j.Get(args,key);
   if(n<0 || j.Type(n)==JV_NULL)
      return true;
   if(!j.IsWhole(n))
     {
      err="argument '"+key+"' must be an integer";
      return false;
     }
   val=j.Int(n);
   present=true;
   return true;
  }

//--- symbol must exist; optionally also be selected into Market Watch.
//--- rc/err are set on failure.
bool Jarvis_CheckSymbol(const string sym,const bool select,int &rc,string &err)
  {
   int len=StringLen(sym);
   if(len<1 || len>JARVIS_MAX_SYMBOL_LEN)
     {
      rc=JARVIS_RC_INVALID;
      err="invalid symbol name length";
      return false;
     }
   for(int i=0; i<len; i++)
     {
      if(StringGetCharacter(sym,i)<0x20)
        {
         rc=JARVIS_RC_INVALID;
         err="invalid symbol name";
         return false;
        }
     }
   ResetLastError();
   bool custom=false;
   if(!SymbolExist(sym,custom))
     {
      rc=JARVIS_RC_UNKNOWN_SYMBOL;
      err="unknown symbol: "+sym;
      return false;
     }
   if(select)
     {
      ResetLastError();
      if(!SymbolSelect(sym,true))
        {
         rc=GetLastError();
         if(rc==0)
            rc=JARVIS_RC_UNKNOWN_SYMBOL;
         err="symbol cannot be selected: "+sym;
         return false;
        }
     }
   return true;
  }

//--- number of decimals for prices of a symbol; -1 if unknown (=> trimmed output)
int Jarvis_Digits(const string sym)
  {
   long d=0;
   if(sym!="" && SymbolInfoInteger(sym,SYMBOL_DIGITS,d) && d>=0 && d<=10)
      return (int)d;
   return -1;
  }

//--- "demo"/"contest"/"real"; false when no account is logged in
bool Jarvis_TradeMode(string &mode)
  {
   if(AccountInfoInteger(ACCOUNT_LOGIN)==0)
      return false;
   switch((int)AccountInfoInteger(ACCOUNT_TRADE_MODE))
     {
      case ACCOUNT_TRADE_MODE_DEMO:
         mode="demo";
         return true;
      case ACCOUNT_TRADE_MODE_CONTEST:
         mode="contest";
         return true;
      case ACCOUNT_TRADE_MODE_REAL:
         mode="real";
         return true;
     }
   return false;
  }

//+------------------------------------------------------------------+
//| ping                                                             |
//+------------------------------------------------------------------+
int Jarvis_CmdPing(CJson &req,const int args,CJsonWriter &w,string &err)
  {
   string mode;
   if(!Jarvis_TradeMode(mode))
     {
      err="terminal has no connected trading account";
      return JARVIS_RC_INTERNAL;
     }
   bool allowed=(TerminalInfoInteger(TERMINAL_TRADE_ALLOWED)!=0) && (MQLInfoInteger(MQL_TRADE_ALLOWED)!=0);
   w.BeginObject();
   w.PutInt("protocol_version",JARVIS_PROTOCOL_VERSION);
   w.PutStr("ea_version",JARVIS_EA_VERSION);
   w.PutStr("trade_mode",mode);
   w.PutBool("terminal_connected",TerminalInfoInteger(TERMINAL_CONNECTED)!=0);
   w.PutBool("trade_allowed",allowed);
   w.PutInt("server_time",(long)TimeTradeServer());
   w.EndObject();
   return 0;
  }

//+------------------------------------------------------------------+
//| get_price {symbol}                                               |
//+------------------------------------------------------------------+
int Jarvis_CmdGetPrice(CJson &req,const int args,CJsonWriter &w,string &err)
  {
   string sym;
   bool present;
   if(!Jarvis_ArgString(req,args,"symbol",sym,present,err))
      return JARVIS_RC_INVALID;
   if(!present)
      return Jarvis_Invalid(err,"missing argument 'symbol'");
   int rc=0;
   if(!Jarvis_CheckSymbol(sym,true,rc,err))
      return rc;

   MqlTick tick;
   ResetLastError();
   if(!SymbolInfoTick(sym,tick))
      return Jarvis_MtFail(err,"no tick data for symbol: "+sym);
   if(tick.bid<=0.0 && tick.ask<=0.0)
     {
      err="no quotes available for symbol: "+sym;
      return JARVIS_RC_NO_TICK;
     }
   long spread=0;
   SymbolInfoInteger(sym,SYMBOL_SPREAD,spread);
   int digits=Jarvis_Digits(sym);

   w.BeginObject();
   w.PutStr("symbol",sym);
   w.PutNum("bid",tick.bid,digits);
   w.PutNum("ask",tick.ask,digits);
   w.PutNum("last",tick.last,digits);
   w.PutInt("spread_points",spread);
   w.PutInt("digits",(digits<0)?0:digits);
   w.PutInt("time",(long)tick.time);
   w.EndObject();
   return 0;
  }

//+------------------------------------------------------------------+
//| get_symbol {symbol}                                              |
//| Trading specification. trade_allowed is true for full, long-only |
//| and short-only symbols (order_send enforces the direction) and   |
//| false for close-only or disabled ones.                           |
//+------------------------------------------------------------------+
int Jarvis_CmdGetSymbol(CJson &req,const int args,CJsonWriter &w,string &err)
  {
   string sym;
   bool present;
   if(!Jarvis_ArgString(req,args,"symbol",sym,present,err))
      return JARVIS_RC_INVALID;
   if(!present)
      return Jarvis_Invalid(err,"missing argument 'symbol'");
   int rc=0;
   if(!Jarvis_CheckSymbol(sym,true,rc,err))
      return rc;

   long digits=0,stops=0,tmode=0;
   SymbolInfoInteger(sym,SYMBOL_DIGITS,digits);
   SymbolInfoInteger(sym,SYMBOL_TRADE_STOPS_LEVEL,stops);
   SymbolInfoInteger(sym,SYMBOL_TRADE_MODE,tmode);
   bool allowed=(tmode==SYMBOL_TRADE_MODE_FULL || tmode==SYMBOL_TRADE_MODE_LONGONLY || tmode==SYMBOL_TRADE_MODE_SHORTONLY);

   w.BeginObject();
   w.PutStr("symbol",sym);
   w.PutStr("description",SymbolInfoString(sym,SYMBOL_DESCRIPTION));
   w.PutInt("digits",digits);
   w.PutNum("point",SymbolInfoDouble(sym,SYMBOL_POINT));
   w.PutNum("volume_min",SymbolInfoDouble(sym,SYMBOL_VOLUME_MIN));
   w.PutNum("volume_max",SymbolInfoDouble(sym,SYMBOL_VOLUME_MAX));
   w.PutNum("volume_step",SymbolInfoDouble(sym,SYMBOL_VOLUME_STEP));
   w.PutNum("contract_size",SymbolInfoDouble(sym,SYMBOL_TRADE_CONTRACT_SIZE));
   w.PutNum("tick_value",SymbolInfoDouble(sym,SYMBOL_TRADE_TICK_VALUE));
   w.PutNum("tick_size",SymbolInfoDouble(sym,SYMBOL_TRADE_TICK_SIZE));
   w.PutInt("stops_level",stops);
   w.PutBool("trade_allowed",allowed);
   w.PutStr("currency_profit",SymbolInfoString(sym,SYMBOL_CURRENCY_PROFIT));
   w.EndObject();
   return 0;
  }

//+------------------------------------------------------------------+
//| get_account {}                                                   |
//| Never includes login, name, server or any other identity field.  |
//+------------------------------------------------------------------+
int Jarvis_CmdGetAccount(CJson &req,const int args,CJsonWriter &w,string &err)
  {
   string mode;
   if(!Jarvis_TradeMode(mode))
     {
      err="terminal has no connected trading account";
      return JARVIS_RC_INTERNAL;
     }
   double margin=AccountInfoDouble(ACCOUNT_MARGIN);
   double level=(margin>0.0)?AccountInfoDouble(ACCOUNT_MARGIN_LEVEL):0.0;

   w.BeginObject();
   w.PutStr("currency",AccountInfoString(ACCOUNT_CURRENCY));
   w.PutNum("balance",AccountInfoDouble(ACCOUNT_BALANCE));
   w.PutNum("equity",AccountInfoDouble(ACCOUNT_EQUITY));
   w.PutNum("profit",AccountInfoDouble(ACCOUNT_PROFIT));
   w.PutNum("margin",margin);
   w.PutNum("free_margin",AccountInfoDouble(ACCOUNT_MARGIN_FREE));
   w.PutNum("margin_level",level);
   w.PutInt("leverage",AccountInfoInteger(ACCOUNT_LEVERAGE));
   w.PutStr("trade_mode",mode);
   w.EndObject();
   return 0;
  }

//+------------------------------------------------------------------+
//| get_positions {symbol?}                                          |
//+------------------------------------------------------------------+
int Jarvis_CmdGetPositions(CJson &req,const int args,CJsonWriter &w,string &err)
  {
   string filter;
   bool present;
   if(!Jarvis_ArgString(req,args,"symbol",filter,present,err))
      return JARVIS_RC_INVALID;
   int rc=0;
   if(present && !Jarvis_CheckSymbol(filter,false,rc,err))
      return rc;

   w.BeginObject();
   w.Key("positions");
   w.BeginArray();
   int total=PositionsTotal();
   for(int i=0; i<total; i++)
     {
      ulong ticket=PositionGetTicket(i);
      if(ticket==0)
         continue;   // position vanished between PositionsTotal and now
      string sym=PositionGetString(POSITION_SYMBOL);
      if(present && StringCompare(sym,filter,false)!=0)
         continue;
      long type=PositionGetInteger(POSITION_TYPE);
      if(type!=POSITION_TYPE_BUY && type!=POSITION_TYPE_SELL)
         continue;
      int digits=Jarvis_Digits(sym);
      w.BeginObject();
      w.PutUInt("ticket",ticket);
      w.PutStr("symbol",sym);
      w.PutStr("side",(type==POSITION_TYPE_BUY)?"buy":"sell");
      w.PutNum("volume",PositionGetDouble(POSITION_VOLUME));
      w.PutNum("price_open",PositionGetDouble(POSITION_PRICE_OPEN),digits);
      w.PutNum("price_current",PositionGetDouble(POSITION_PRICE_CURRENT),digits);
      w.PutNumOrNull("sl",PositionGetDouble(POSITION_SL),digits);
      w.PutNumOrNull("tp",PositionGetDouble(POSITION_TP),digits);
      w.PutNum("profit",PositionGetDouble(POSITION_PROFIT));
      w.PutNum("swap",PositionGetDouble(POSITION_SWAP));
      w.PutInt("magic",PositionGetInteger(POSITION_MAGIC));
      w.PutStr("comment",PositionGetString(POSITION_COMMENT));
      w.PutInt("time",PositionGetInteger(POSITION_TIME));
      w.EndObject();
     }
   w.EndArray();
   w.EndObject();
   return 0;
  }

//--- ORDER_TYPE_* -> wire name; "" for types the protocol does not carry
string Jarvis_OrderTypeName(const long t)
  {
   switch((int)t)
     {
      case ORDER_TYPE_BUY_LIMIT:
         return "buy_limit";
      case ORDER_TYPE_SELL_LIMIT:
         return "sell_limit";
      case ORDER_TYPE_BUY_STOP:
         return "buy_stop";
      case ORDER_TYPE_SELL_STOP:
         return "sell_stop";
      case ORDER_TYPE_BUY_STOP_LIMIT:
         return "buy_stop_limit";
      case ORDER_TYPE_SELL_STOP_LIMIT:
         return "sell_stop_limit";
     }
   return "";
  }

//+------------------------------------------------------------------+
//| get_orders {symbol?}  (pending orders)                           |
//+------------------------------------------------------------------+
int Jarvis_CmdGetOrders(CJson &req,const int args,CJsonWriter &w,string &err)
  {
   string filter;
   bool present;
   if(!Jarvis_ArgString(req,args,"symbol",filter,present,err))
      return JARVIS_RC_INVALID;
   int rc=0;
   if(present && !Jarvis_CheckSymbol(filter,false,rc,err))
      return rc;

   w.BeginObject();
   w.Key("orders");
   w.BeginArray();
   int total=OrdersTotal();
   for(int i=0; i<total; i++)
     {
      ulong ticket=OrderGetTicket(i);
      if(ticket==0)
         continue;
      string sym=OrderGetString(ORDER_SYMBOL);
      if(present && StringCompare(sym,filter,false)!=0)
         continue;
      string tname=Jarvis_OrderTypeName(OrderGetInteger(ORDER_TYPE));
      if(tname=="")
         continue;
      int digits=Jarvis_Digits(sym);
      w.BeginObject();
      w.PutUInt("ticket",ticket);
      w.PutStr("symbol",sym);
      w.PutStr("type",tname);
      w.PutNum("volume_initial",OrderGetDouble(ORDER_VOLUME_INITIAL));
      w.PutNum("volume_current",OrderGetDouble(ORDER_VOLUME_CURRENT));
      w.PutNum("price_open",OrderGetDouble(ORDER_PRICE_OPEN),digits);
      w.PutNumOrNull("sl",OrderGetDouble(ORDER_SL),digits);
      w.PutNumOrNull("tp",OrderGetDouble(ORDER_TP),digits);
      w.PutInt("time_setup",OrderGetInteger(ORDER_TIME_SETUP));
      w.PutStr("comment",OrderGetString(ORDER_COMMENT));
      w.EndObject();
     }
   w.EndArray();
   w.EndObject();
   return 0;
  }

//--- DEAL_TYPE_* -> wire name
string Jarvis_DealTypeName(const long t)
  {
   switch((int)t)
     {
      case DEAL_TYPE_BUY:
         return "buy";
      case DEAL_TYPE_SELL:
         return "sell";
      case DEAL_TYPE_BALANCE:
         return "balance";
      case DEAL_TYPE_CREDIT:
         return "credit";
     }
   return "other";
  }

//--- DEAL_ENTRY_* -> wire name; "" for entries the protocol does not carry
string Jarvis_DealEntryName(const long e)
  {
   switch((int)e)
     {
      case DEAL_ENTRY_IN:
         return "in";
      case DEAL_ENTRY_OUT:
         return "out";
      case DEAL_ENTRY_INOUT:
         return "inout";
      case DEAL_ENTRY_OUT_BY:
         return "out_by";
     }
   return "";
  }

//+------------------------------------------------------------------+
//| get_history {from,to,symbol?,limit?}                             |
//| Newest deals first. truncated=true when more deals matched.      |
//+------------------------------------------------------------------+
int Jarvis_CmdGetHistory(CJson &req,const int args,CJsonWriter &w,string &err)
  {
   long from=0,to=0,limit=0;
   bool hasFrom,hasTo,hasLimit,hasSym;
   string filter;
   if(!Jarvis_ArgInt(req,args,"from",from,hasFrom,err))
      return JARVIS_RC_INVALID;
   if(!Jarvis_ArgInt(req,args,"to",to,hasTo,err))
      return JARVIS_RC_INVALID;
   if(!Jarvis_ArgInt(req,args,"limit",limit,hasLimit,err))
      return JARVIS_RC_INVALID;
   if(!Jarvis_ArgString(req,args,"symbol",filter,hasSym,err))
      return JARVIS_RC_INVALID;
   if(!hasFrom)
      return Jarvis_Invalid(err,"missing argument 'from'");
   if(!hasTo)
      return Jarvis_Invalid(err,"missing argument 'to'");
   if(from<0 || to<0 || from>JARVIS_MAX_DATETIME || to>JARVIS_MAX_DATETIME)
      return Jarvis_Invalid(err,"'from' and 'to' must be Unix seconds in a valid range");
   if(from>to)
      return Jarvis_Invalid(err,"'from' must not be after 'to'");
   if(!hasLimit)
      limit=JARVIS_HISTORY_LIMIT_DEFAULT;
   if(limit<1 || limit>JARVIS_HISTORY_LIMIT_MAX)
      return Jarvis_Invalid(err,StringFormat("'limit' must be between 1 and %d",JARVIS_HISTORY_LIMIT_MAX));
   int rc=0;
   if(hasSym && !Jarvis_CheckSymbol(filter,false,rc,err))
      return rc;

   ResetLastError();
   if(!HistorySelect((datetime)from,(datetime)to))
      return Jarvis_MtFail(err,"history is not available for the requested range");
   int total=HistoryDealsTotal();

   w.BeginObject();
   w.Key("deals");
   w.BeginArray();
   int written=0;
   bool truncated=false;
   for(int i=total-1; i>=0; i--)
     {
      ulong ticket=HistoryDealGetTicket(i);
      if(ticket==0)
         continue;
      string entry=Jarvis_DealEntryName(HistoryDealGetInteger(ticket,DEAL_ENTRY));
      if(entry=="")
         continue;   // account-state records etc. are not trades
      string sym=HistoryDealGetString(ticket,DEAL_SYMBOL);
      if(hasSym && StringCompare(sym,filter,false)!=0)
         continue;
      if(written>=limit)
        {
         truncated=true;   // one more match exists beyond the limit
         break;
        }
      int digits=Jarvis_Digits(sym);
      w.BeginObject();
      w.PutUInt("ticket",ticket);
      w.PutUInt("order",(ulong)HistoryDealGetInteger(ticket,DEAL_ORDER));
      w.PutUInt("position_id",(ulong)HistoryDealGetInteger(ticket,DEAL_POSITION_ID));
      w.PutStr("symbol",sym);
      w.PutStr("type",Jarvis_DealTypeName(HistoryDealGetInteger(ticket,DEAL_TYPE)));
      w.PutStr("entry",entry);
      w.PutNum("volume",HistoryDealGetDouble(ticket,DEAL_VOLUME));
      w.PutNum("price",HistoryDealGetDouble(ticket,DEAL_PRICE),digits);
      w.PutNum("profit",HistoryDealGetDouble(ticket,DEAL_PROFIT));
      w.PutNum("commission",HistoryDealGetDouble(ticket,DEAL_COMMISSION));
      w.PutNum("swap",HistoryDealGetDouble(ticket,DEAL_SWAP));
      w.PutInt("time",HistoryDealGetInteger(ticket,DEAL_TIME));
      w.PutStr("comment",HistoryDealGetString(ticket,DEAL_COMMENT));
      w.EndObject();
      written++;
     }
   w.EndArray();
   w.PutBool("truncated",truncated);
   w.EndObject();
   return 0;
  }

#endif // JARVIS_READ_HANDLERS_MQH
