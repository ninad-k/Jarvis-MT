//+------------------------------------------------------------------+
//| Json.mqh - small JSON parser and writer for the Jarvis bridge.   |
//|                                                                  |
//| Parser: text -> tagged value tree held in a flat node pool       |
//|   (no pointers, nothing to leak, every index is bounds-checked). |
//| Writer: builds compact JSON text with correct string escaping.   |
//|                                                                  |
//| Limits (defense against hostile or broken input):                |
//|   JSON_MAX_TEXT   total input characters                         |
//|   JSON_MAX_DEPTH  nesting depth                                  |
//|   JSON_MAX_NODES  total values                                   |
//+------------------------------------------------------------------+
#ifndef JARVIS_JSON_MQH
#define JARVIS_JSON_MQH

#define JSON_MAX_TEXT   65536
#define JSON_MAX_DEPTH  32
#define JSON_MAX_NODES  4096
#define JSON_MAX_STRING 16384

enum ENUM_JV_TYPE
  {
   JV_NULL   = 0,
   JV_BOOL   = 1,
   JV_NUMBER = 2,
   JV_STRING = 3,
   JV_ARRAY  = 4,
   JV_OBJECT = 5
  };

//--- one parsed value. Children are linked through first/last/next.
struct JsonNode
  {
   int               type;
   bool              b;
   double            num;
   long              inum;
   bool              isint;
   string            s;
   string            key;
   int               first;
   int               last;
   int               next;
   int               count;
  };

//+------------------------------------------------------------------+
//| Parser and read access                                           |
//+------------------------------------------------------------------+
class CJson
  {
private:
   JsonNode          m_n[];
   int               m_cnt;
   string            m_t;
   int               m_len;
   int               m_pos;
   string            m_err;

   bool              Fail(const string msg)
     {
      if(m_err=="")
         m_err=StringFormat("invalid JSON: %s at offset %d",msg,m_pos);
      return false;
     }

   int               NewNode(const int type)
     {
      if(m_cnt>=JSON_MAX_NODES)
        {
         Fail("too many values");
         return -1;
        }
      if(m_cnt>=ArraySize(m_n))
        {
         if(ArrayResize(m_n,m_cnt+64)<0)
           {
            Fail("out of memory");
            return -1;
           }
        }
      int i=m_cnt;
      m_cnt++;
      m_n[i].type=type;
      m_n[i].b=false;
      m_n[i].num=0.0;
      m_n[i].inum=0;
      m_n[i].isint=false;
      m_n[i].s="";
      m_n[i].key="";
      m_n[i].first=-1;
      m_n[i].last=-1;
      m_n[i].next=-1;
      m_n[i].count=0;
      return i;
     }

   void              Append(const int parent,const int child)
     {
      if(m_n[parent].last<0)
         m_n[parent].first=child;
      else
         m_n[m_n[parent].last].next=child;
      m_n[parent].last=child;
      m_n[parent].count++;
     }

   void              SkipWs()
     {
      while(m_pos<m_len)
        {
         ushort c=StringGetCharacter(m_t,m_pos);
         if(c==' ' || c=='\t' || c=='\n' || c=='\r')
            m_pos++;
         else
            break;
        }
     }

   int               HexVal(const ushort c)
     {
      if(c>='0' && c<='9')
         return c-'0';
      if(c>='a' && c<='f')
         return c-'a'+10;
      if(c>='A' && c<='F')
         return c-'A'+10;
      return -1;
     }

   //--- m_pos is on the opening quote
   bool              ParseString(string &out)
     {
      out="";
      m_pos++;
      int start=m_pos;
      while(true)
        {
         if(m_pos>=m_len)
            return Fail("unterminated string");
         ushort c=StringGetCharacter(m_t,m_pos);
         if(c=='"')
           {
            if(m_pos>start)
               out+=StringSubstr(m_t,start,m_pos-start);
            m_pos++;
            return true;
           }
         if(c<0x20)
            return Fail("control character in string");
         if(c=='\\')
           {
            if(m_pos>start)
               out+=StringSubstr(m_t,start,m_pos-start);
            m_pos++;
            if(m_pos>=m_len)
               return Fail("unterminated escape");
            ushort e=StringGetCharacter(m_t,m_pos);
            switch(e)
              {
               case '"':
                  out+="\"";
                  break;
               case '\\':
                  out+="\\";
                  break;
               case '/':
                  out+="/";
                  break;
               case 'b':
                  out+=ShortToString(8);
                  break;
               case 'f':
                  out+=ShortToString(12);
                  break;
               case 'n':
                  out+=ShortToString(10);
                  break;
               case 'r':
                  out+=ShortToString(13);
                  break;
               case 't':
                  out+=ShortToString(9);
                  break;
               case 'u':
                 {
                  if(m_pos+4>=m_len)
                     return Fail("truncated \\u escape");
                  int v=0;
                  for(int k=1; k<=4; k++)
                    {
                     int h=HexVal(StringGetCharacter(m_t,m_pos+k));
                     if(h<0)
                        return Fail("bad \\u escape");
                     v=(v<<4)|h;
                    }
                  m_pos+=4;
                  //--- surrogates are kept as UTF-16 units; a valid pair stays valid
                  out+=ShortToString((ushort)v);
                  break;
                 }
               default:
                  return Fail("bad escape");
              }
            m_pos++;
            start=m_pos;
            if(StringLen(out)>JSON_MAX_STRING)
               return Fail("string too long");
            continue;
           }
         m_pos++;
         if(m_pos-start>JSON_MAX_STRING)
            return Fail("string too long");
        }
      return false;
     }

   int               ParseNumber()
     {
      int start=m_pos;
      bool isint=true;
      if(StringGetCharacter(m_t,m_pos)=='-')
         m_pos++;
      if(m_pos>=m_len)
        {
         Fail("bad number");
         return -1;
        }
      ushort c=StringGetCharacter(m_t,m_pos);
      if(c=='0')
         m_pos++;
      else
         if(c>='1' && c<='9')
           {
            while(m_pos<m_len && StringGetCharacter(m_t,m_pos)>='0' && StringGetCharacter(m_t,m_pos)<='9')
               m_pos++;
           }
         else
           {
            Fail("unexpected character");
            return -1;
           }
      if(m_pos<m_len && StringGetCharacter(m_t,m_pos)=='.')
        {
         isint=false;
         m_pos++;
         int d0=m_pos;
         while(m_pos<m_len && StringGetCharacter(m_t,m_pos)>='0' && StringGetCharacter(m_t,m_pos)<='9')
            m_pos++;
         if(m_pos==d0)
           {
            Fail("bad number");
            return -1;
           }
        }
      if(m_pos<m_len && (StringGetCharacter(m_t,m_pos)=='e' || StringGetCharacter(m_t,m_pos)=='E'))
        {
         isint=false;
         m_pos++;
         if(m_pos<m_len && (StringGetCharacter(m_t,m_pos)=='+' || StringGetCharacter(m_t,m_pos)=='-'))
            m_pos++;
         int e0=m_pos;
         while(m_pos<m_len && StringGetCharacter(m_t,m_pos)>='0' && StringGetCharacter(m_t,m_pos)<='9')
            m_pos++;
         if(m_pos==e0)
           {
            Fail("bad number");
            return -1;
           }
        }
      string tok=StringSubstr(m_t,start,m_pos-start);
      double d=StringToDouble(tok);
      if(!MathIsValidNumber(d))
        {
         Fail("number out of range");
         return -1;
        }
      int n=NewNode(JV_NUMBER);
      if(n<0)
         return -1;
      m_n[n].num=d;
      if(isint && StringLen(tok)<=18)
        {
         m_n[n].isint=true;
         m_n[n].inum=StringToInteger(tok);
        }
      return n;
     }

   bool              Literal(const string word)
     {
      int l=StringLen(word);
      if(m_pos+l<=m_len && StringSubstr(m_t,m_pos,l)==word)
        {
         m_pos+=l;
         return true;
        }
      return false;
     }

   int               ParseValue(const int depth)
     {
      if(depth>JSON_MAX_DEPTH)
        {
         Fail("nesting too deep");
         return -1;
        }
      SkipWs();
      if(m_pos>=m_len)
        {
         Fail("unexpected end of input");
         return -1;
        }
      ushort c=StringGetCharacter(m_t,m_pos);
      if(c=='{')
         return ParseObject(depth);
      if(c=='[')
         return ParseArray(depth);
      if(c=='"')
        {
         string s;
         if(!ParseString(s))
            return -1;
         int n=NewNode(JV_STRING);
         if(n<0)
            return -1;
         m_n[n].s=s;
         return n;
        }
      if(c=='t')
        {
         if(!Literal("true"))
           {
            Fail("unexpected character");
            return -1;
           }
         int n=NewNode(JV_BOOL);
         if(n>=0)
            m_n[n].b=true;
         return n;
        }
      if(c=='f')
        {
         if(!Literal("false"))
           {
            Fail("unexpected character");
            return -1;
           }
         return NewNode(JV_BOOL);
        }
      if(c=='n')
        {
         if(!Literal("null"))
           {
            Fail("unexpected character");
            return -1;
           }
         return NewNode(JV_NULL);
        }
      if(c=='-' || (c>='0' && c<='9'))
         return ParseNumber();
      Fail("unexpected character");
      return -1;
     }

   int               ParseArray(const int depth)
     {
      int arr=NewNode(JV_ARRAY);
      if(arr<0)
         return -1;
      m_pos++;
      SkipWs();
      if(m_pos<m_len && StringGetCharacter(m_t,m_pos)==']')
        {
         m_pos++;
         return arr;
        }
      while(true)
        {
         int child=ParseValue(depth+1);
         if(child<0)
            return -1;
         Append(arr,child);
         SkipWs();
         if(m_pos>=m_len)
           {
            Fail("unterminated array");
            return -1;
           }
         ushort c=StringGetCharacter(m_t,m_pos);
         m_pos++;
         if(c==',')
            continue;
         if(c==']')
            return arr;
         m_pos--;
         Fail("expected ',' or ']'");
         return -1;
        }
      return -1;
     }

   int               ParseObject(const int depth)
     {
      int obj=NewNode(JV_OBJECT);
      if(obj<0)
         return -1;
      m_pos++;
      SkipWs();
      if(m_pos<m_len && StringGetCharacter(m_t,m_pos)=='}')
        {
         m_pos++;
         return obj;
        }
      while(true)
        {
         SkipWs();
         if(m_pos>=m_len || StringGetCharacter(m_t,m_pos)!='"')
           {
            Fail("expected string key");
            return -1;
           }
         string key;
         if(!ParseString(key))
            return -1;
         SkipWs();
         if(m_pos>=m_len || StringGetCharacter(m_t,m_pos)!=':')
           {
            Fail("expected ':'");
            return -1;
           }
         m_pos++;
         int child=ParseValue(depth+1);
         if(child<0)
            return -1;
         m_n[child].key=key;
         Append(obj,child);
         SkipWs();
         if(m_pos>=m_len)
           {
            Fail("unterminated object");
            return -1;
           }
         ushort c=StringGetCharacter(m_t,m_pos);
         m_pos++;
         if(c==',')
            continue;
         if(c=='}')
            return obj;
         m_pos--;
         Fail("expected ',' or '}'");
         return -1;
        }
      return -1;
     }

public:
                     CJson():m_cnt(0),m_len(0),m_pos(0)
     {
     }

   //--- parse a whole document; false on any error (see Error())
   bool              Parse(const string text)
     {
      m_cnt=0;
      m_err="";
      m_pos=0;
      m_t=text;
      m_len=StringLen(text);
      if(m_len<=0)
         return Fail("empty input");
      if(m_len>JSON_MAX_TEXT)
         return Fail("input too large");
      int root=ParseValue(0);
      if(root<0)
        {
         m_cnt=0;
         if(m_err=="")
            Fail("parse error");
         return false;
        }
      SkipWs();
      if(m_pos<m_len)
        {
         m_cnt=0;
         return Fail("trailing characters");
        }
      return true;
     }

   string            Error()
     {
      return m_err;
     }

   //--- read access. Every accessor tolerates invalid indexes.
   int               Root()
     {
      return (m_cnt>0)?0:-1;
     }
   bool              Valid(const int n)
     {
      return n>=0 && n<m_cnt;
     }
   int               Type(const int n)
     {
      return Valid(n)?m_n[n].type:-1;
     }
   bool              IsObject(const int n)
     {
      return Type(n)==JV_OBJECT;
     }
   bool              IsString(const int n)
     {
      return Type(n)==JV_STRING;
     }
   bool              IsNumber(const int n)
     {
      return Type(n)==JV_NUMBER;
     }
   int               Count(const int n)
     {
      return (Valid(n) && (m_n[n].type==JV_ARRAY || m_n[n].type==JV_OBJECT))?m_n[n].count:0;
     }
   //--- member of an object; the last duplicate wins (same as serde_json)
   int               Get(const int obj,const string key)
     {
      if(Type(obj)!=JV_OBJECT)
         return -1;
      int found=-1;
      int c=m_n[obj].first;
      int guard=0;
      while(c>=0 && c<m_cnt && guard<=JSON_MAX_NODES)
        {
         if(m_n[c].key==key)
            found=c;
         c=m_n[c].next;
         guard++;
        }
      return found;
     }
   //--- i-th element of an array (or member of an object)
   int               At(const int n,const int idx)
     {
      if(!Valid(n) || idx<0 || idx>=Count(n))
         return -1;
      int c=m_n[n].first;
      for(int i=0; i<idx && c>=0; i++)
         c=m_n[c].next;
      return c;
     }
   string            Str(const int n)
     {
      return (Type(n)==JV_STRING)?m_n[n].s:"";
     }
   bool              Bool(const int n)
     {
      return (Type(n)==JV_BOOL)?m_n[n].b:false;
     }
   double            Num(const int n)
     {
      return (Type(n)==JV_NUMBER)?m_n[n].num:0.0;
     }
   //--- true for a number written as an integer, or a whole float within 2^53
   bool              IsWhole(const int n)
     {
      if(Type(n)!=JV_NUMBER)
         return false;
      if(m_n[n].isint)
         return true;
      return MathAbs(m_n[n].num)<=9007199254740992.0 && m_n[n].num==MathFloor(m_n[n].num);
     }
   long              Int(const int n)
     {
      if(Type(n)!=JV_NUMBER)
         return 0;
      return m_n[n].isint?m_n[n].inum:(long)m_n[n].num;
     }
  };

//+------------------------------------------------------------------+
//| Writer                                                           |
//|   w.BeginObject(); w.PutStr("a","x"); w.EndObject();             |
//| Commas are inserted automatically. Output is built in chunks so  |
//| large replies do not pay for quadratic string concatenation.     |
//+------------------------------------------------------------------+
class CJsonWriter
  {
private:
   string            m_chunks[];
   int               m_nch;
   string            m_cur;
   bool              m_has[64];
   int               m_depth;
   bool              m_afterKey;

   void              Flush()
     {
      if(m_cur=="")
         return;
      if(ArrayResize(m_chunks,m_nch+1,64)>0)
        {
         m_chunks[m_nch]=m_cur;
         m_nch++;
        }
      m_cur="";
     }
   void              Put(const string s)
     {
      m_cur+=s;
      if(StringLen(m_cur)>=4096)
         Flush();
     }
   //--- comma handling before every value or key
   void              Before()
     {
      if(m_afterKey)
        {
         m_afterKey=false;
         return;
        }
      if(m_depth>0)
        {
         if(m_has[m_depth])
            Put(",");
         m_has[m_depth]=true;
        }
     }
   void              Open(const string ch)
     {
      Before();
      Put(ch);
      if(m_depth<62)
         m_depth++;
      m_has[m_depth]=false;
     }
   void              Close(const string ch)
     {
      Put(ch);
      if(m_depth>0)
         m_depth--;
     }

public:
                     CJsonWriter():m_nch(0),m_cur(""),m_depth(0),m_afterKey(false)
     {
      m_has[0]=false;
     }

   void              Reset()
     {
      ArrayFree(m_chunks);
      m_nch=0;
      m_cur="";
      m_depth=0;
      m_afterKey=false;
      m_has[0]=false;
     }

   //--- JSON string escaping: quotes, backslash and control characters
   static string     Escape(const string s)
     {
      int n=StringLen(s);
      bool need=false;
      int i;
      for(i=0; i<n; i++)
        {
         ushort c=StringGetCharacter(s,i);
         if(c<0x20 || c=='"' || c=='\\')
           {
            need=true;
            break;
           }
        }
      if(!need)
         return s;
      string out="";
      int start=0;
      for(i=0; i<n; i++)
        {
         ushort c=StringGetCharacter(s,i);
         if(c>=0x20 && c!='"' && c!='\\')
            continue;
         if(i>start)
            out+=StringSubstr(s,start,i-start);
         switch(c)
           {
            case '"':
               out+="\\\"";
               break;
            case '\\':
               out+="\\\\";
               break;
            case 8:
               out+="\\b";
               break;
            case 9:
               out+="\\t";
               break;
            case 10:
               out+="\\n";
               break;
            case 12:
               out+="\\f";
               break;
            case 13:
               out+="\\r";
               break;
            default:
               out+=StringFormat("\\u%04x",c);
               break;
           }
         start=i+1;
        }
      if(start<n)
         out+=StringSubstr(s,start,n-start);
      return out;
     }

   //--- number text. digits<0: up to 8 decimals with trailing zeros trimmed.
   //--- digits>=0: exactly that many decimals. Never emits nan/inf.
   static string     FormatNum(const double v,const int digits=-1)
     {
      if(!MathIsValidNumber(v))
         return "0";
      int d=digits;
      bool trim=false;
      if(d<0 || d>16)
        {
         d=8;
         trim=true;
        }
      string s=DoubleToString(v,d);
      if(trim)
        {
         int l=StringLen(s);
         if(StringFind(s,".")>=0)
           {
            while(l>0 && StringGetCharacter(s,l-1)=='0')
               l--;
            if(l>0 && StringGetCharacter(s,l-1)=='.')
               l--;
            s=StringSubstr(s,0,l);
           }
         if(s=="-0" || s=="" || s=="-")
            s="0";
        }
      return s;
     }

   void              BeginObject()
     {
      Open("{");
     }
   void              EndObject()
     {
      Close("}");
     }
   void              BeginArray()
     {
      Open("[");
     }
   void              EndArray()
     {
      Close("]");
     }
   //--- start a member inside an object; follow with a value or Begin*
   void              Key(const string k)
     {
      Before();
      Put("\""+Escape(k)+"\":");
      m_afterKey=true;
     }

   //--- bare values (array elements, or after Key)
   void              ValStr(const string v)
     {
      Before();
      Put("\""+Escape(v)+"\"");
     }
   void              ValRaw(const string rawjson)
     {
      Before();
      Put(rawjson);
     }

   //--- "key": value members
   void              PutStr(const string k,const string v)
     {
      Key(k);
      ValStr(v);
     }
   void              PutInt(const string k,const long v)
     {
      Key(k);
      ValRaw(IntegerToString(v));
     }
   void              PutUInt(const string k,const ulong v)
     {
      Key(k);
      ValRaw((string)v);
     }
   void              PutNum(const string k,const double v,const int digits=-1)
     {
      Key(k);
      ValRaw(FormatNum(v,digits));
     }
   //--- 0 becomes null (unset sl/tp)
   void              PutNumOrNull(const string k,const double v,const int digits=-1)
     {
      Key(k);
      if(v==0.0 || !MathIsValidNumber(v))
         ValRaw("null");
      else
         ValRaw(FormatNum(v,digits));
     }
   void              PutBool(const string k,const bool v)
     {
      Key(k);
      ValRaw(v?"true":"false");
     }
   void              PutNull(const string k)
     {
      Key(k);
      ValRaw("null");
     }
   void              PutRaw(const string k,const string rawjson)
     {
      Key(k);
      ValRaw(rawjson);
     }

   string            ToString()
     {
      string r="";
      for(int i=0; i<m_nch; i++)
         r+=m_chunks[i];
      r+=m_cur;
      return r;
     }
  };

#endif // JARVIS_JSON_MQH
