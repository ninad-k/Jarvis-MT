//+------------------------------------------------------------------+
//| Auth.mqh - message authentication of bridge protocol 2.          |
//|                                                                  |
//| Every message (request and reply) is a ZeroMQ multipart message  |
//| of exactly two frames:                                           |
//|   frame 1: lowercase hex HMAC-SHA256(key, frame 2)  (64 chars)   |
//|   frame 2: the JSON payload, byte for byte as sent               |
//| The key is 32 random bytes, hex in MQL5\Files\JarvisMT\bridge.key|
//| (created by scripts\install-ea.ps1). A request carries "ts"      |
//| (Unix ms of the sender's clock). A request is refused (and never |
//| executed) when                                                   |
//|   - it is not exactly two frames, or the MAC is wrong/missing,   |
//|   - its ts is more than 30 s away from this clock, or            |
//|   - its id was already used in the last 60 s.                    |
//| HMAC is implemented here on top of CryptEncode(CRYPT_HASH_SHA256)|
//| and proven against RFC 4231 in JarvisSelfTest.mq5.               |
//| Spec: docs/protocol.md, docs/adr/0016-authenticated-bridge-protocol.md |
//+------------------------------------------------------------------+
#ifndef JARVIS_AUTH_MQH
#define JARVIS_AUTH_MQH

#define JARVIS_RC_AUTH_FAILED        (-4)
#define JARVIS_AUTH_ERROR            "authentication failed"
#define JARVIS_KEY_FILE              "JarvisMT\\bridge.key"      // inside MQL5\Files
#define JARVIS_KEY_BYTES             32
#define JARVIS_MAX_SKEW_MS           30000
#define JARVIS_REPLAY_WINDOW_MS      60000
#define JARVIS_REPLAY_CAPACITY       4096
#define JARVIS_HMAC_BLOCK            64
#define JARVIS_MAX_TS                9007199254740991     // 2^53-1, what a JSON number can hold exactly

//--- outcome of the freshness/replay check
#define JARVIS_FRESH_OK              0
#define JARVIS_FRESH_STALE           1    // ts too far from this clock
#define JARVIS_FRESH_REPLAYED        2    // id already used in the last 60 s
#define JARVIS_FRESH_FULL            3    // 4096 ids in the last 60 s: refuse until some expire
#define JARVIS_FRESH_NO_TS           4    // ts missing or not an integer

//--- the shared key. Never printed, never logged.
uchar g_jarvisKey[];
bool  g_jarvisKeyReady=false;

//--- test hooks (the self-test sets them; the EA never does)
long  g_jarvisClockOverrideMs=0;         // != 0: Jarvis_NowMs() returns this
bool  g_jarvisMonoOverrideOn=false;
ulong g_jarvisMonoOverrideMs=0;          // used when g_jarvisMonoOverrideOn

//--- clock anchors for Jarvis_NowMs()
long  g_jarvisGmtAnchor=0;
ulong g_jarvisTickAnchor=0;

//--- ids seen in the last 60 s: a ring ordered by arrival (oldest at g_jarvisSeenHead)
string g_jarvisSeenId[JARVIS_REPLAY_CAPACITY];
ulong  g_jarvisSeenAt[JARVIS_REPLAY_CAPACITY];
int    g_jarvisSeenHead=0;
int    g_jarvisSeenCount=0;

//+------------------------------------------------------------------+
//| Bytes, hex, constant-time compare                                |
//+------------------------------------------------------------------+
//--- UTF-8 bytes of a string, without a terminating zero
void Jarvis_Utf8(const string text,uchar &bytes[])
  {
   ArrayResize(bytes,0);
   if(text=="")
      return;
   int n=StringToCharArray(text,bytes,0,WHOLE_ARRAY,CP_UTF8);   // includes the terminating zero
   if(n<=0)
     {
      ArrayResize(bytes,0);
      return;
     }
   ArrayResize(bytes,n-1);
  }

//--- ASCII bytes of a string (hex digits)
void Jarvis_AsciiBytes(const string text,uchar &bytes[])
  {
   int n=StringLen(text);
   ArrayResize(bytes,n);
   for(int i=0; i<n; i++)
      bytes[i]=(uchar)StringGetCharacter(text,i);
  }

string Jarvis_HexEncode(const uchar &bytes[])
  {
   string out="";
   int n=ArraySize(bytes);
   for(int i=0; i<n; i++)
      out+=StringFormat("%02x",(int)bytes[i]);
   return out;
  }

int Jarvis_HexNibble(const ushort c)
  {
   if(c>='0' && c<='9')
      return (int)(c-'0');
   if(c>='a' && c<='f')
      return (int)(c-'a')+10;
   if(c>='A' && c<='F')
      return (int)(c-'A')+10;
   return -1;
  }

//--- decode hex (either case). false on odd length or a non-hex character.
bool Jarvis_HexDecode(const string hex,uchar &bytes[])
  {
   ArrayResize(bytes,0);
   int n=StringLen(hex);
   if(n%2!=0)
      return false;
   ArrayResize(bytes,n/2);
   for(int i=0; i<n/2; i++)
     {
      int hi=Jarvis_HexNibble(StringGetCharacter(hex,2*i));
      int lo=Jarvis_HexNibble(StringGetCharacter(hex,2*i+1));
      if(hi<0 || lo<0)
        {
         ArrayResize(bytes,0);
         return false;
        }
      bytes[i]=(uchar)((hi<<4)|lo);
     }
   return true;
  }

//--- equal length and equal content, without an early exit on the first difference
bool Jarvis_ConstTimeEqual(const uchar &a[],const uchar &b[])
  {
   int n=ArraySize(a);
   if(n!=ArraySize(b))
      return false;
   int diff=0;
   for(int i=0; i<n; i++)
      diff|=((int)a[i]^(int)b[i]);
   return diff==0;
  }

//+------------------------------------------------------------------+
//| SHA-256 and HMAC-SHA256 (RFC 2104 / RFC 4231), block size 64     |
//+------------------------------------------------------------------+
bool Jarvis_Sha256(const uchar &data[],uchar &digest[])
  {
   uchar nokey[];
   ArrayResize(digest,0);
   int n=CryptEncode(CRYPT_HASH_SHA256,data,nokey,digest);
   if(n!=32 || ArraySize(digest)!=32)
     {
      ArrayResize(digest,0);
      return false;
     }
   return true;
  }

bool Jarvis_HmacSha256(const uchar &key[],const uchar &message[],uchar &mac[])
  {
   ArrayResize(mac,0);
   //--- keys longer than the block are hashed first, shorter ones are zero padded
   uchar k[];
   if(ArraySize(key)>JARVIS_HMAC_BLOCK)
     {
      if(!Jarvis_Sha256(key,k))
         return false;
     }
   else
      if(ArraySize(key)>0)
         ArrayCopy(k,key);
   int klen=ArraySize(k);
   ArrayResize(k,JARVIS_HMAC_BLOCK);
   for(int i=klen; i<JARVIS_HMAC_BLOCK; i++)
      k[i]=0;

   int mlen=ArraySize(message);
   uchar inner[];
   ArrayResize(inner,JARVIS_HMAC_BLOCK+mlen);
   for(int i=0; i<JARVIS_HMAC_BLOCK; i++)
      inner[i]=(uchar)(k[i]^0x36);
   if(mlen>0)
      ArrayCopy(inner,message,JARVIS_HMAC_BLOCK,0,mlen);
   uchar innerHash[];
   if(!Jarvis_Sha256(inner,innerHash))
      return false;

   uchar outer[];
   ArrayResize(outer,JARVIS_HMAC_BLOCK+32);
   for(int i=0; i<JARVIS_HMAC_BLOCK; i++)
      outer[i]=(uchar)(k[i]^0x5c);
   ArrayCopy(outer,innerHash,JARVIS_HMAC_BLOCK,0,32);
   //--- best effort: do not leave key-derived blocks around
   ArrayInitialize(k,0);
   ArrayInitialize(inner,0);
   return Jarvis_Sha256(outer,mac);
  }

//--- lowercase hex HMAC-SHA256(key, message), "" on failure
string Jarvis_HmacHex(const uchar &key[],const uchar &message[])
  {
   uchar mac[];
   if(!Jarvis_HmacSha256(key,message,mac))
      return "";
   return Jarvis_HexEncode(mac);
  }

//+------------------------------------------------------------------+
//| The shared key                                                   |
//+------------------------------------------------------------------+
//--- accept a 64-character hex key (surrounding blanks ignored, either case)
bool Jarvis_SetKeyHex(const string text,string &err)
  {
   g_jarvisKeyReady=false;
   ArrayResize(g_jarvisKey,0);
   string hex=text;
   StringTrimLeft(hex);
   StringTrimRight(hex);
   if(StringLen(hex)!=2*JARVIS_KEY_BYTES)
     {
      err=StringFormat("the key must be %d hexadecimal characters (32 random bytes), found %d characters",
                       2*JARVIS_KEY_BYTES,StringLen(hex));
      return false;
     }
   uchar bytes[];
   if(!Jarvis_HexDecode(hex,bytes))
     {
      err="the key contains a character that is not hexadecimal";
      return false;
     }
   bool allZero=true;
   for(int i=0; i<JARVIS_KEY_BYTES; i++)
      if(bytes[i]!=0)
         allZero=false;
   if(allZero)
     {
      err="the key is all zeros";
      return false;
     }
   ArrayCopy(g_jarvisKey,bytes);
   ArrayInitialize(bytes,0);
   g_jarvisKeyReady=true;
   return true;
  }

//--- read the key from a file inside MQL5\Files (first non-empty line)
bool Jarvis_LoadKeyFile(const string path,string &err)
  {
   ResetLastError();
   int h=FileOpen(path,FILE_READ|FILE_TXT|FILE_ANSI|FILE_SHARE_READ);
   if(h==INVALID_HANDLE)
     {
      err=StringFormat("cannot open the key file MQL5\\Files\\%s (error %d). Run scripts\\install-ea.ps1 to create it",
                       path,GetLastError());
      return false;
     }
   string line="";
   while(!FileIsEnding(h))
     {
      line=FileReadString(h);
      StringTrimLeft(line);
      StringTrimRight(line);
      if(line!="")
         break;
     }
   FileClose(h);
   string why="";
   if(!Jarvis_SetKeyHex(line,why))
     {
      err=StringFormat("the key file MQL5\\Files\\%s is not usable: %s",path,why);
      line="";
      return false;
     }
   line="";
   return true;
  }

//+------------------------------------------------------------------+
//| Frames                                                           |
//+------------------------------------------------------------------+
//--- the frame-1 bytes expected for this payload (64 ASCII hex digits)
bool Jarvis_ExpectedMacFrame(const uchar &payload[],uchar &frame[])
  {
   ArrayResize(frame,0);
   if(!g_jarvisKeyReady)
      return false;
   string hex=Jarvis_HmacHex(g_jarvisKey,payload);
   if(StringLen(hex)!=2*32)
      return false;
   Jarvis_AsciiBytes(hex,frame);
   return true;
  }

//--- true when frame 1 is the correct MAC of frame 2 (constant time)
bool Jarvis_VerifyFrames(const uchar &macFrame[],const uchar &payload[])
  {
   uchar expected[];
   if(!Jarvis_ExpectedMacFrame(payload,expected))
      return false;
   return Jarvis_ConstTimeEqual(macFrame,expected);
  }

//+------------------------------------------------------------------+
//| Clocks                                                           |
//+------------------------------------------------------------------+
//--- monotonic milliseconds (replay window bookkeeping; immune to clock changes)
ulong Jarvis_MonoMs()
  {
   if(g_jarvisMonoOverrideOn)
      return g_jarvisMonoOverrideMs;
   return GetTickCount64();
  }

//+------------------------------------------------------------------+
//| Unix time in milliseconds, comparable with the client's "ts".    |
//| Precision: TimeGMT() has one-second resolution. The milliseconds |
//| are added from GetTickCount64() since the moment this function   |
//| first saw the second change, so with the 20 ms timer calling it  |
//| the error is at most about 25 ms; after a long idle gap, up to   |
//| 1 s. Either is far below the 30 s skew limit.                    |
//+------------------------------------------------------------------+
long Jarvis_NowMs()
  {
   if(g_jarvisClockOverrideMs!=0)
      return g_jarvisClockOverrideMs;
   long gmt=(long)TimeGMT();
   ulong tick=GetTickCount64();
   if(gmt!=g_jarvisGmtAnchor)
     {
      g_jarvisGmtAnchor=gmt;
      g_jarvisTickAnchor=tick;
     }
   long frac=(long)(tick-g_jarvisTickAnchor);
   if(frac>999)
      frac=999;
   return gmt*1000+frac;
  }

//+------------------------------------------------------------------+
//| Freshness and replay                                             |
//+------------------------------------------------------------------+
void Jarvis_ReplayReset()
  {
   for(int i=0; i<JARVIS_REPLAY_CAPACITY; i++)
      g_jarvisSeenId[i]="";
   g_jarvisSeenHead=0;
   g_jarvisSeenCount=0;
  }

//--- forget ids older than the window
void Jarvis_ReplayExpire(const ulong mono)
  {
   while(g_jarvisSeenCount>0 &&
         (long)(mono-g_jarvisSeenAt[g_jarvisSeenHead])>JARVIS_REPLAY_WINDOW_MS)
     {
      g_jarvisSeenId[g_jarvisSeenHead]="";
      g_jarvisSeenHead=(g_jarvisSeenHead+1)%JARVIS_REPLAY_CAPACITY;
      g_jarvisSeenCount--;
     }
  }

//+------------------------------------------------------------------+
//| Check one request that already passed the MAC check.             |
//| Returns JARVIS_FRESH_OK and remembers the id, or the reason.     |
//+------------------------------------------------------------------+
int Jarvis_FreshnessCheck(const string id,const long ts)
  {
   if(ts<0 || ts>JARVIS_MAX_TS)
      return JARVIS_FRESH_STALE;
   long off=ts-Jarvis_NowMs();
   if(off<0)
      off=-off;
   if(off>JARVIS_MAX_SKEW_MS)
      return JARVIS_FRESH_STALE;
   ulong mono=Jarvis_MonoMs();
   Jarvis_ReplayExpire(mono);
   for(int i=0; i<g_jarvisSeenCount; i++)
      if(g_jarvisSeenId[(g_jarvisSeenHead+i)%JARVIS_REPLAY_CAPACITY]==id)
         return JARVIS_FRESH_REPLAYED;
   if(g_jarvisSeenCount>=JARVIS_REPLAY_CAPACITY)
      return JARVIS_FRESH_FULL;
   int at=(g_jarvisSeenHead+g_jarvisSeenCount)%JARVIS_REPLAY_CAPACITY;
   g_jarvisSeenId[at]=id;
   g_jarvisSeenAt[at]=mono;
   g_jarvisSeenCount++;
   return JARVIS_FRESH_OK;
  }

#endif // JARVIS_AUTH_MQH
