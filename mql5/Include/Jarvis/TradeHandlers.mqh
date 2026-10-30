//+------------------------------------------------------------------+
//| TradeHandlers.mqh - the write commands.                          |
//|                                                                  |
//|   order_send, position_modify, position_close,                   |
//|   position_close_partial, order_cancel                           |
//|                                                                  |
//| Rules (docs/protocol.md, SPEC core rules):                       |
//|   - Writes happen only in response to a request. No OnTick logic.|
//|   - Every request is fully validated BEFORE CTrade is touched;   |
//|     invalid input => ok:false, retcode -1, nothing is sent.      |
//|   - Volumes are never rounded: a volume that is not a multiple of|
//|     the symbol's step is rejected.                               |
//|   - Real accounts are refused (retcode -3) unless the EA input   |
//|     InpAllowReal is true, and when Algo Trading is off.          |
//|   - The last 64 write replies are cached by request id. A write  |
//|     whose id is in the cache gets the cached reply verbatim and  |
//|     is NOT executed again.                                       |
//| Wire format: crates/bridge-client/src/protocol.rs                |
//+------------------------------------------------------------------+
#ifndef JARVIS_TRADE_HANDLERS_MQH
#define JARVIS_TRADE_HANDLERS_MQH

#include <Trade/Trade.mqh>
#include "ReadHandlers.mqh"

#define JARVIS_RC_WRITE_REFUSED   (-3)
#define JARVIS_DEFAULT_DEVIATION  20
#define JARVIS_MAX_DEVIATION      10000
#define JARVIS_MAX_COMMENT_LEN    31
#define JARVIS_VOLUME_TOLERANCE   1e-8
#define JARVIS_CACHE_SIZE         64
#define JARVIS_RC_TRADE_ERROR     10011   // TRADE_RETCODE_ERROR: request failed before a server answer

//--- set by the EA from its InpAllowReal input; the self-test script leaves it false
bool g_jarvisAllowReal=false;

//--- outcome of one write request. ok=false carries "error", ok=true carries "data".
struct JarvisTradeOut
  {
   bool              ok;
   int               retcode;   // trade retcode for executed requests, -1 / -3 otherwise
   string            error;
   string            data;      // TradeResult JSON object when ok
  };

//+------------------------------------------------------------------+
//| Reply cache (ring of the last JARVIS_CACHE_SIZE write replies)   |
//+------------------------------------------------------------------+
string g_jarvisCacheId[JARVIS_CACHE_SIZE];
string g_jarvisCacheReply[JARVIS_CACHE_SIZE];
bool   g_jarvisCacheOk[JARVIS_CACHE_SIZE];
int    g_jarvisCacheRc[JARVIS_CACHE_SIZE];
int    g_jarvisCacheNext=0;
int    g_jarvisCacheCount=0;
ulong  g_jarvisCacheHits=0;

//--- index of the cached reply for this request id, or -1
int Jarvis_CacheFind(const string id)
  {
   for(int i=0; i<g_jarvisCacheCount; i++)
      if(g_jarvisCacheId[i]==id)
         return i;
   return -1;
  }

void Jarvis_CacheStore(const string id,const string reply,const bool ok,const int retcode)
  {
   if(id=="" || Jarvis_CacheFind(id)>=0)
      return;
   g_jarvisCacheId[g_jarvisCacheNext]=id;
   g_jarvisCacheReply[g_jarvisCacheNext]=reply;
   g_jarvisCacheOk[g_jarvisCacheNext]=ok;
   g_jarvisCacheRc[g_jarvisCacheNext]=retcode;
   g_jarvisCacheNext=(g_jarvisCacheNext+1)%JARVIS_CACHE_SIZE;
   if(g_jarvisCacheCount<JARVIS_CACHE_SIZE)
      g_jarvisCacheCount++;
  }

//+------------------------------------------------------------------+
//| Small helpers                                                    |
//+------------------------------------------------------------------+
void Jarvis_Reject(JarvisTradeOut &out,const string msg,const int retcode=JARVIS_RC_INVALID)
  {
   out.ok=false;
   out.retcode=retcode;
   out.error=msg;
   out.data="";
  }

string Jarvis_Fmt(const double v,const int digits)
  {
   return (digits>=0)?DoubleToString(v,digits):CJsonWriter::FormatNum(v);
  }

bool Jarvis_IsHedging()
  {
   return (ENUM_ACCOUNT_MARGIN_MODE)AccountInfoInteger(ACCOUNT_MARGIN_MODE)==ACCOUNT_MARGIN_MODE_RETAIL_HEDGING;
  }

//--- optional finite number argument; null counts as absent
bool Jarvis_ArgNum(CJson &j,const int args,const string key,double &val,bool &present,string &err)
  {
   present=false;
   val=0.0;
   int n=j.Get(args,key);
   if(n<0 || j.Type(n)==JV_NULL)
      return true;
   if(j.Type(n)!=JV_NUMBER || !MathIsValidNumber(j.Num(n)))
     {
      err="argument '"+key+"' must be a finite number";
      return false;
     }
   val=j.Num(n);
   present=true;
   return true;
  }

//--- required ticket argument (positive whole number)
bool Jarvis_ArgTicket(CJson &j,const int args,ulong &ticket,string &err)
  {
   long v=0;
   bool present;
   if(!Jarvis_ArgInt(j,args,"ticket",v,present,err))
      return false;
   if(!present)
     {
      err="missing argument 'ticket'";
      return false;
     }
   if(v<=0)
     {
      err="argument 'ticket' must be a positive integer";
      return false;
     }
   ticket=(ulong)v;
   return true;
  }

//--- optional deviation in points (default JARVIS_DEFAULT_DEVIATION)
bool Jarvis_ArgDeviation(CJson &j,const int args,ulong &dev,string &err)
  {
   long v=0;
   bool present;
   dev=JARVIS_DEFAULT_DEVIATION;
   if(!Jarvis_ArgInt(j,args,"deviation_points",v,present,err))
      return false;
   if(!present)
      return true;
   if(v<0 || v>JARVIS_MAX_DEVIATION)
     {
      err=StringFormat("argument 'deviation_points' must be between 0 and %d",JARVIS_MAX_DEVIATION);
      return false;
     }
   dev=(ulong)v;
   return true;
  }

//+------------------------------------------------------------------+
//| Safety gate: may this terminal / account be written to at all?   |
//| returns 0, or JARVIS_RC_WRITE_REFUSED with err set               |
//+------------------------------------------------------------------+
int Jarvis_WriteGate(string &err)
  {
   if(MQLInfoInteger(MQL_TESTER)!=0)
     {
      err="writes refused: not supported in the strategy tester";
      return JARVIS_RC_WRITE_REFUSED;
     }
   string mode;
   if(!Jarvis_TradeMode(mode))
     {
      err="writes refused: terminal has no connected trading account";
      return JARVIS_RC_WRITE_REFUSED;
     }
   if(mode=="real" && !g_jarvisAllowReal)
     {
      err="writes refused on a real account (InpAllowReal is false)";
      return JARVIS_RC_WRITE_REFUSED;
     }
   if(TerminalInfoInteger(TERMINAL_TRADE_ALLOWED)==0)
     {
      err="writes refused: Algo Trading is disabled in the terminal (enable Algo Trading)";
      return JARVIS_RC_WRITE_REFUSED;
     }
   if(MQLInfoInteger(MQL_TRADE_ALLOWED)==0)
     {
      err="writes refused: trading is not allowed for this EA (enable Algo Trading for the EA)";
      return JARVIS_RC_WRITE_REFUSED;
     }
   if(AccountInfoInteger(ACCOUNT_TRADE_ALLOWED)==0)
     {
      err="writes refused: trading is disabled for this account";
      return JARVIS_RC_WRITE_REFUSED;
     }
   if(AccountInfoInteger(ACCOUNT_TRADE_EXPERT)==0)
     {
      err="writes refused: automated trading is disabled for this account";
      return JARVIS_RC_WRITE_REFUSED;
     }
   return 0;
  }

//+------------------------------------------------------------------+
//| Validation helpers (all return false with err set)               |
//+------------------------------------------------------------------+

//--- volume must be within [min,max] and a whole multiple of the step; never rounded
bool Jarvis_CheckVolume(const string sym,const double vol,string &err)
  {
   double vmin=SymbolInfoDouble(sym,SYMBOL_VOLUME_MIN);
   double vmax=SymbolInfoDouble(sym,SYMBOL_VOLUME_MAX);
   double step=SymbolInfoDouble(sym,SYMBOL_VOLUME_STEP);
   if(!MathIsValidNumber(vol) || vol<=0.0)
     {
      err="volume must be a positive number";
      return false;
     }
   if(vmin<=0.0 || step<=0.0 || vmax<vmin)
     {
      err="volume limits are not available for "+sym;
      return false;
     }
   if(vol<vmin-JARVIS_VOLUME_TOLERANCE)
     {
      err="volume "+CJsonWriter::FormatNum(vol)+" is below the minimum "+CJsonWriter::FormatNum(vmin);
      return false;
     }
   if(vol>vmax+JARVIS_VOLUME_TOLERANCE)
     {
      err="volume "+CJsonWriter::FormatNum(vol)+" is above the maximum "+CJsonWriter::FormatNum(vmax);
      return false;
     }
   if(MathAbs(vol-MathRound(vol/step)*step)>JARVIS_VOLUME_TOLERANCE)
     {
      err="volume "+CJsonWriter::FormatNum(vol)+" is not a multiple of the volume step "+CJsonWriter::FormatNum(step);
      return false;
     }
   return true;
  }

//--- a price must be finite, positive and already expressed in the symbol's digits
bool Jarvis_CheckPriceFormat(const string sym,const string name,const double p,string &err)
  {
   if(!MathIsValidNumber(p) || p<=0.0)
     {
      err=name+" must be a positive number";
      return false;
     }
   int digits=Jarvis_Digits(sym);
   double point=SymbolInfoDouble(sym,SYMBOL_POINT);
   if(digits>=0 && point>0.0 && MathAbs(p-NormalizeDouble(p,digits))>point*0.001)
     {
      err=StringFormat("%s has more than %d decimals for %s",name,digits,sym);
      return false;
     }
   return true;
  }

//--- minimum distance (price units) from the reference price: stops level, at least one point
double Jarvis_MinDistance(const string sym)
  {
   long stops=0;
   SymbolInfoInteger(sym,SYMBOL_TRADE_STOPS_LEVEL,stops);
   double point=SymbolInfoDouble(sym,SYMBOL_POINT);
   return (double)MathMax((long)1,stops)*point;
  }

//--- sl/tp relative to the reference price. A value of 0 means "not set" and is skipped.
//--- buy: sl below, tp above. sell: sl above, tp below. At least the minimum distance away.
bool Jarvis_CheckLevels(const string sym,const bool isBuy,const double ref,const string refName,
                        const double sl,const double tp,string &err)
  {
   double point=SymbolInfoDouble(sym,SYMBOL_POINT);
   double dist=Jarvis_MinDistance(sym);
   double eps=point*0.001;
   int digits=Jarvis_Digits(sym);
   long stops=0;
   SymbolInfoInteger(sym,SYMBOL_TRADE_STOPS_LEVEL,stops);
   string need=StringFormat("by at least %d points (stops level)",(int)MathMax((long)1,stops));
   if(sl>0.0)
     {
      if(isBuy && sl>ref-dist+eps)
        {
         err="sl "+Jarvis_Fmt(sl,digits)+" must be below the "+refName+" "+Jarvis_Fmt(ref,digits)+" "+need;
         return false;
        }
      if(!isBuy && sl<ref+dist-eps)
        {
         err="sl "+Jarvis_Fmt(sl,digits)+" must be above the "+refName+" "+Jarvis_Fmt(ref,digits)+" "+need;
         return false;
        }
     }
   if(tp>0.0)
     {
      if(isBuy && tp<ref+dist-eps)
        {
         err="tp "+Jarvis_Fmt(tp,digits)+" must be above the "+refName+" "+Jarvis_Fmt(ref,digits)+" "+need;
         return false;
        }
      if(!isBuy && tp>ref-dist+eps)
        {
         err="tp "+Jarvis_Fmt(tp,digits)+" must be below the "+refName+" "+Jarvis_Fmt(ref,digits)+" "+need;
         return false;
        }
     }
   return true;
  }

//--- symbol exists, is selected and has quotes; fills the current tick
bool Jarvis_PrepareSymbol(const string sym,MqlTick &tick,int &rc,string &err)
  {
   if(!Jarvis_CheckSymbol(sym,true,rc,err))
     {
      rc=JARVIS_RC_INVALID;   // writes report every bad argument as -1
      return false;
     }
   ResetLastError();
   if(!SymbolInfoTick(sym,tick) || tick.bid<=0.0 || tick.ask<=0.0)
     {
      rc=JARVIS_RC_INVALID;
      err="no quotes available for symbol: "+sym;
      return false;
     }
   return true;
  }

//+------------------------------------------------------------------+
//| CTrade plumbing                                                  |
//+------------------------------------------------------------------+
void Jarvis_SetFilling(CTrade &trade,const string sym)
  {
   if(!trade.SetTypeFillingBySymbol(sym))
      trade.SetTypeFilling(ORDER_FILLING_RETURN);
  }

//--- build the TradeResult data / error from what CTrade recorded
void Jarvis_FinishTrade(CTrade &trade,const string sym,JarvisTradeOut &out)
  {
   int lastErr=GetLastError();
   uint rc=trade.ResultRetcode();
   string desc="";
   if(rc==0)
     {
      //--- CTrade refused before anything reached the server
      rc=JARVIS_RC_TRADE_ERROR;
      desc=StringFormat("request failed before reaching the server (error %d)",lastErr);
     }
   else
      desc=trade.ResultRetcodeDescription();

   bool ok=(rc==TRADE_RETCODE_PLACED || rc==TRADE_RETCODE_DONE || rc==TRADE_RETCODE_DONE_PARTIAL);
   out.ok=ok;
   out.retcode=(int)rc;
   if(!ok)
     {
      out.error=StringFormat("trade failed: %s (%u)",desc,rc);
      out.data="";
      return;
     }
   int digits=Jarvis_Digits(sym);
   CJsonWriter w;
   w.BeginObject();
   w.PutInt("retcode",(long)rc);
   w.PutStr("retcode_description",desc);
   w.PutUInt("order",trade.ResultOrder());
   w.PutUInt("deal",trade.ResultDeal());
   w.PutNum("volume",trade.ResultVolume());
   w.PutNum("price",trade.ResultPrice(),digits);
   w.PutStr("comment",trade.ResultComment());
   w.EndObject();
   out.error="";
   out.data=w.ToString();
  }

//+------------------------------------------------------------------+
//| order_send {symbol,side,volume,type,price?,sl?,tp?,              |
//|             deviation_points?,magic,comment}                     |
//+------------------------------------------------------------------+
void Jarvis_CmdOrderSend(CJson &req,const int args,JarvisTradeOut &out)
  {
   string err="";
   string sym,sideS,typeS;
   bool has;
   if(!Jarvis_ArgString(req,args,"symbol",sym,has,err) || !has)
     {
      Jarvis_Reject(out,(err=="")?"missing argument 'symbol'":err);
      return;
     }
   if(!Jarvis_ArgString(req,args,"side",sideS,has,err) || !has)
     {
      Jarvis_Reject(out,(err=="")?"missing argument 'side'":err);
      return;
     }
   if(sideS!="buy" && sideS!="sell")
     {
      Jarvis_Reject(out,"argument 'side' must be \"buy\" or \"sell\"");
      return;
     }
   if(!Jarvis_ArgString(req,args,"type",typeS,has,err) || !has)
     {
      Jarvis_Reject(out,(err=="")?"missing argument 'type'":err);
      return;
     }
   if(typeS!="market" && typeS!="limit" && typeS!="stop")
     {
      Jarvis_Reject(out,"argument 'type' must be \"market\", \"limit\" or \"stop\"");
      return;
     }
   double volume=0.0,price=0.0,sl=0.0,tp=0.0;
   bool hasVol,hasPrice,hasSl,hasTp;
   if(!Jarvis_ArgNum(req,args,"volume",volume,hasVol,err) ||
      !Jarvis_ArgNum(req,args,"price",price,hasPrice,err) ||
      !Jarvis_ArgNum(req,args,"sl",sl,hasSl,err) ||
      !Jarvis_ArgNum(req,args,"tp",tp,hasTp,err))
     {
      Jarvis_Reject(out,err);
      return;
     }
   if(!hasVol)
     {
      Jarvis_Reject(out,"missing argument 'volume'");
      return;
     }
   ulong deviation=0;
   if(!Jarvis_ArgDeviation(req,args,deviation,err))
     {
      Jarvis_Reject(out,err);
      return;
     }
   long magic=0;
   bool hasMagic;
   if(!Jarvis_ArgInt(req,args,"magic",magic,hasMagic,err))
     {
      Jarvis_Reject(out,err);
      return;
     }
   if(!hasMagic)
     {
      Jarvis_Reject(out,"missing argument 'magic'");
      return;
     }
   if(magic<0)
     {
      Jarvis_Reject(out,"argument 'magic' must not be negative");
      return;
     }
   string comment="";
   int cn=req.Get(args,"comment");
   if(cn>=0 && req.Type(cn)!=JV_NULL)
     {
      if(req.Type(cn)!=JV_STRING)
        {
         Jarvis_Reject(out,"argument 'comment' must be a string");
         return;
        }
      comment=req.Str(cn);
     }
   if(StringLen(comment)>JARVIS_MAX_COMMENT_LEN)
     {
      Jarvis_Reject(out,StringFormat("comment is longer than %d characters",JARVIS_MAX_COMMENT_LEN));
      return;
     }

   //--- price rules by type
   bool market=(typeS=="market");
   if(market && hasPrice)
     {
      Jarvis_Reject(out,"market orders must not carry a price");
      return;
     }
   if(!market && !hasPrice)
     {
      Jarvis_Reject(out,"limit and stop orders need a price");
      return;
     }
   if(hasSl && sl<0.0)
     {
      Jarvis_Reject(out,"sl must not be negative");
      return;
     }
   if(hasTp && tp<0.0)
     {
      Jarvis_Reject(out,"tp must not be negative");
      return;
     }

   //--- safety gate (after the arguments are known to be well formed)
   int rc=Jarvis_WriteGate(err);
   if(rc!=0)
     {
      Jarvis_Reject(out,err,rc);
      return;
     }

   //--- market state
   MqlTick tick;
   if(!Jarvis_PrepareSymbol(sym,tick,rc,err))
     {
      Jarvis_Reject(out,err);
      return;
     }
   bool isBuy=(sideS=="buy");
   long tmode=SymbolInfoInteger(sym,SYMBOL_TRADE_MODE);
   if(tmode==SYMBOL_TRADE_MODE_DISABLED || tmode==SYMBOL_TRADE_MODE_CLOSEONLY)
     {
      Jarvis_Reject(out,"trading is not allowed for symbol (disabled or close-only): "+sym);
      return;
     }
   if(tmode==SYMBOL_TRADE_MODE_LONGONLY && !isBuy)
     {
      Jarvis_Reject(out,"symbol allows long positions only: "+sym);
      return;
     }
   if(tmode==SYMBOL_TRADE_MODE_SHORTONLY && isBuy)
     {
      Jarvis_Reject(out,"symbol allows short positions only: "+sym);
      return;
     }
   if(!Jarvis_CheckVolume(sym,volume,err))
     {
      Jarvis_Reject(out,err);
      return;
     }
   int digits=Jarvis_Digits(sym);
   double dist=Jarvis_MinDistance(sym);
   double eps=SymbolInfoDouble(sym,SYMBOL_POINT)*0.001;
   if(hasPrice && !Jarvis_CheckPriceFormat(sym,"price",price,err))
     {
      Jarvis_Reject(out,err);
      return;
     }
   if(hasSl && sl>0.0 && !Jarvis_CheckPriceFormat(sym,"sl",sl,err))
     {
      Jarvis_Reject(out,err);
      return;
     }
   if(hasTp && tp>0.0 && !Jarvis_CheckPriceFormat(sym,"tp",tp,err))
     {
      Jarvis_Reject(out,err);
      return;
     }

   ENUM_ORDER_TYPE otype;
   double levelRef;
   string levelName;
   if(market)
     {
      otype=isBuy?ORDER_TYPE_BUY:ORDER_TYPE_SELL;
      levelRef=isBuy?tick.bid:tick.ask;   // MT5 measures stops of an open position from the close price
      levelName=isBuy?"bid":"ask";
     }
   else
     {
      if(typeS=="limit")
         otype=isBuy?ORDER_TYPE_BUY_LIMIT:ORDER_TYPE_SELL_LIMIT;
      else
         otype=isBuy?ORDER_TYPE_BUY_STOP:ORDER_TYPE_SELL_STOP;
      //--- the pending price against the market: buy limit below ask, buy stop above ask,
      //--- sell limit above bid, sell stop below bid, each at least the minimum distance away
      bool below=(isBuy && typeS=="limit") || (!isBuy && typeS=="stop");
      double ref=isBuy?tick.ask:tick.bid;
      string refName=isBuy?"ask":"bid";
      if(below && price>ref-dist+eps)
        {
         Jarvis_Reject(out,StringFormat("%s %s price %s must be below the %s %s by at least %d points (stops level)",
                                        sideS,typeS,Jarvis_Fmt(price,digits),refName,Jarvis_Fmt(ref,digits),(int)MathRound(dist/SymbolInfoDouble(sym,SYMBOL_POINT))));
         return;
        }
      if(!below && price<ref+dist-eps)
        {
         Jarvis_Reject(out,StringFormat("%s %s price %s must be above the %s %s by at least %d points (stops level)",
                                        sideS,typeS,Jarvis_Fmt(price,digits),refName,Jarvis_Fmt(ref,digits),(int)MathRound(dist/SymbolInfoDouble(sym,SYMBOL_POINT))));
         return;
        }
      levelRef=price;
      levelName="order price";
     }
   if(!Jarvis_CheckLevels(sym,isBuy,levelRef,levelName,hasSl?sl:0.0,hasTp?tp:0.0,err))
     {
      Jarvis_Reject(out,err);
      return;
     }

   //--- all checks passed: hand over to CTrade
   CTrade trade;
   trade.SetExpertMagicNumber((ulong)magic);
   trade.SetDeviationInPoints(deviation);
   Jarvis_SetFilling(trade,sym);
   double slv=hasSl?sl:0.0;
   double tpv=hasTp?tp:0.0;
   ResetLastError();
   if(market)
      trade.PositionOpen(sym,otype,volume,isBuy?tick.ask:tick.bid,slv,tpv,comment);
   else
      trade.OrderOpen(sym,otype,volume,0.0,price,slv,tpv,ORDER_TIME_GTC,0,comment);
   Jarvis_FinishTrade(trade,sym,out);
  }

//+------------------------------------------------------------------+
//| position_modify {ticket,sl?,tp?}                                 |
//| absent = keep the current value, 0 = remove it                   |
//+------------------------------------------------------------------+
void Jarvis_CmdPositionModify(CJson &req,const int args,JarvisTradeOut &out)
  {
   string err="";
   ulong ticket=0;
   if(!Jarvis_ArgTicket(req,args,ticket,err))
     {
      Jarvis_Reject(out,err);
      return;
     }
   double sl=0.0,tp=0.0;
   bool hasSl,hasTp;
   if(!Jarvis_ArgNum(req,args,"sl",sl,hasSl,err) || !Jarvis_ArgNum(req,args,"tp",tp,hasTp,err))
     {
      Jarvis_Reject(out,err);
      return;
     }
   if(!hasSl && !hasTp)
     {
      Jarvis_Reject(out,"at least one of 'sl' and 'tp' is required");
      return;
     }
   if((hasSl && sl<0.0) || (hasTp && tp<0.0))
     {
      Jarvis_Reject(out,"sl and tp must not be negative (use 0 to remove)");
      return;
     }
   int rc=Jarvis_WriteGate(err);
   if(rc!=0)
     {
      Jarvis_Reject(out,err,rc);
      return;
     }

   if(!PositionSelectByTicket(ticket))
     {
      Jarvis_Reject(out,StringFormat("position not found: %I64u",ticket));
      return;
     }
   string sym=PositionGetString(POSITION_SYMBOL);
   bool isBuy=(PositionGetInteger(POSITION_TYPE)==POSITION_TYPE_BUY);
   double curSl=PositionGetDouble(POSITION_SL);
   double curTp=PositionGetDouble(POSITION_TP);
   MqlTick tick;
   if(!Jarvis_PrepareSymbol(sym,tick,rc,err))
     {
      Jarvis_Reject(out,err);
      return;
     }
   double eps=SymbolInfoDouble(sym,SYMBOL_POINT)*0.001;
   //--- values that are not changing are not re-validated against the moving market
   bool slChanges=hasSl && MathAbs(sl-curSl)>eps;
   bool tpChanges=hasTp && MathAbs(tp-curTp)>eps;
   if(slChanges && sl>0.0 && !Jarvis_CheckPriceFormat(sym,"sl",sl,err))
     {
      Jarvis_Reject(out,err);
      return;
     }
   if(tpChanges && tp>0.0 && !Jarvis_CheckPriceFormat(sym,"tp",tp,err))
     {
      Jarvis_Reject(out,err);
      return;
     }
   double ref=isBuy?tick.bid:tick.ask;
   if(!Jarvis_CheckLevels(sym,isBuy,ref,isBuy?"bid":"ask",slChanges?sl:0.0,tpChanges?tp:0.0,err))
     {
      Jarvis_Reject(out,err);
      return;
     }

   CTrade trade;
   trade.SetExpertMagicNumber((ulong)PositionGetInteger(POSITION_MAGIC));
   ResetLastError();
   trade.PositionModify(ticket,hasSl?sl:curSl,hasTp?tp:curTp);
   Jarvis_FinishTrade(trade,sym,out);
  }

//+------------------------------------------------------------------+
//| position_close {ticket,deviation_points?}                        |
//+------------------------------------------------------------------+
void Jarvis_CmdPositionClose(CJson &req,const int args,JarvisTradeOut &out)
  {
   string err="";
   ulong ticket=0,deviation=0;
   if(!Jarvis_ArgTicket(req,args,ticket,err) || !Jarvis_ArgDeviation(req,args,deviation,err))
     {
      Jarvis_Reject(out,err);
      return;
     }
   int rc=Jarvis_WriteGate(err);
   if(rc!=0)
     {
      Jarvis_Reject(out,err,rc);
      return;
     }
   if(!PositionSelectByTicket(ticket))
     {
      Jarvis_Reject(out,StringFormat("position not found: %I64u",ticket));
      return;
     }
   string sym=PositionGetString(POSITION_SYMBOL);
   MqlTick tick;
   if(!Jarvis_PrepareSymbol(sym,tick,rc,err))
     {
      Jarvis_Reject(out,err);
      return;
     }
   CTrade trade;
   trade.SetExpertMagicNumber((ulong)PositionGetInteger(POSITION_MAGIC));
   trade.SetDeviationInPoints(deviation);
   Jarvis_SetFilling(trade,sym);
   ResetLastError();
   trade.PositionClose(ticket,deviation);
   Jarvis_FinishTrade(trade,sym,out);
  }

//+------------------------------------------------------------------+
//| position_close_partial {ticket,volume,deviation_points?}         |
//| volume must be below the position volume                         |
//+------------------------------------------------------------------+
void Jarvis_CmdPositionClosePartial(CJson &req,const int args,JarvisTradeOut &out)
  {
   string err="";
   ulong ticket=0,deviation=0;
   if(!Jarvis_ArgTicket(req,args,ticket,err) || !Jarvis_ArgDeviation(req,args,deviation,err))
     {
      Jarvis_Reject(out,err);
      return;
     }
   double volume=0.0;
   bool hasVol;
   if(!Jarvis_ArgNum(req,args,"volume",volume,hasVol,err))
     {
      Jarvis_Reject(out,err);
      return;
     }
   if(!hasVol)
     {
      Jarvis_Reject(out,"missing argument 'volume'");
      return;
     }
   int rc=Jarvis_WriteGate(err);
   if(rc!=0)
     {
      Jarvis_Reject(out,err,rc);
      return;
     }
   if(!PositionSelectByTicket(ticket))
     {
      Jarvis_Reject(out,StringFormat("position not found: %I64u",ticket));
      return;
     }
   string sym=PositionGetString(POSITION_SYMBOL);
   long ptype=PositionGetInteger(POSITION_TYPE);
   double posVol=PositionGetDouble(POSITION_VOLUME);
   long magic=PositionGetInteger(POSITION_MAGIC);
   MqlTick tick;
   if(!Jarvis_PrepareSymbol(sym,tick,rc,err))
     {
      Jarvis_Reject(out,err);
      return;
     }
   if(!Jarvis_CheckVolume(sym,volume,err))
     {
      Jarvis_Reject(out,err);
      return;
     }
   if(volume>=posVol-JARVIS_VOLUME_TOLERANCE)
     {
      Jarvis_Reject(out,"volume "+CJsonWriter::FormatNum(volume)+" must be less than the position volume "+
                    CJsonWriter::FormatNum(posVol)+" (use position_close to close it fully)");
      return;
     }
   double vmin=SymbolInfoDouble(sym,SYMBOL_VOLUME_MIN);
   if(posVol-volume<vmin-JARVIS_VOLUME_TOLERANCE)
     {
      Jarvis_Reject(out,"the remaining volume "+CJsonWriter::FormatNum(posVol-volume)+" would be below the minimum "+
                    CJsonWriter::FormatNum(vmin));
      return;
     }

   CTrade trade;
   trade.SetExpertMagicNumber((ulong)magic);
   trade.SetDeviationInPoints(deviation);
   Jarvis_SetFilling(trade,sym);
   ResetLastError();
   if(Jarvis_IsHedging())
      trade.PositionClosePartial(ticket,volume,deviation);
   else
     {
      //--- netting account: the opposite deal reduces the single net position
      bool wasBuy=(ptype==POSITION_TYPE_BUY);
      trade.PositionOpen(sym,wasBuy?ORDER_TYPE_SELL:ORDER_TYPE_BUY,volume,wasBuy?tick.bid:tick.ask,0.0,0.0,"");
     }
   Jarvis_FinishTrade(trade,sym,out);
  }

//+------------------------------------------------------------------+
//| order_cancel {ticket}   (pending orders only)                    |
//+------------------------------------------------------------------+
void Jarvis_CmdOrderCancel(CJson &req,const int args,JarvisTradeOut &out)
  {
   string err="";
   ulong ticket=0;
   if(!Jarvis_ArgTicket(req,args,ticket,err))
     {
      Jarvis_Reject(out,err);
      return;
     }
   int rc=Jarvis_WriteGate(err);
   if(rc!=0)
     {
      Jarvis_Reject(out,err,rc);
      return;
     }
   if(!OrderSelect(ticket))
     {
      Jarvis_Reject(out,StringFormat("pending order not found: %I64u",ticket));
      return;
     }
   string sym=OrderGetString(ORDER_SYMBOL);
   CTrade trade;
   trade.SetExpertMagicNumber((ulong)OrderGetInteger(ORDER_MAGIC));
   ResetLastError();
   trade.OrderDelete(ticket);
   Jarvis_FinishTrade(trade,sym,out);
  }

//+------------------------------------------------------------------+
//| Dispatch one write command. cmd is known to be a write command.  |
//+------------------------------------------------------------------+
void Jarvis_WriteCommand(const string cmd,CJson &req,const int args,JarvisTradeOut &out)
  {
   out.ok=false;
   out.retcode=JARVIS_RC_INVALID;
   out.error="";
   out.data="";
   if(args<0)
     {
      Jarvis_Reject(out,"missing 'args' object");
      return;
     }
   if(cmd=="order_send")
      Jarvis_CmdOrderSend(req,args,out);
   else
      if(cmd=="position_modify")
         Jarvis_CmdPositionModify(req,args,out);
      else
         if(cmd=="position_close")
            Jarvis_CmdPositionClose(req,args,out);
         else
            if(cmd=="position_close_partial")
               Jarvis_CmdPositionClosePartial(req,args,out);
            else
               if(cmd=="order_cancel")
                  Jarvis_CmdOrderCancel(req,args,out);
               else
                  Jarvis_Reject(out,"unknown command: "+cmd,JARVIS_RC_UNKNOWN_CMD);
   if(!out.ok && out.error=="")
      out.error="request failed";
  }

#endif // JARVIS_TRADE_HANDLERS_MQH
