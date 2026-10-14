using System;
using System.Collections;
using System.Collections.Generic;
using System.Globalization;
using System.Text;

namespace Zanshin
{
    /// <summary>Dependency-free JSON: objects are <c>Dictionary&lt;string, object&gt;</c>, arrays <c>List&lt;object&gt;</c>,
    /// numbers <c>double</c> (or <c>long</c> when integral), plus string / bool / null.</summary>
    public static class Json
    {
        public static object Parse(string s)
        {
            if (string.IsNullOrEmpty(s)) return null;
            int i = 0;
            var v = ParseValue(s, ref i);
            SkipWs(s, ref i);
            if (i != s.Length) throw new FormatException("trailing characters in JSON");
            return v;
        }

        public static string Serialize(object v)
        {
            var sb = new StringBuilder();
            Write(sb, v);
            return sb.ToString();
        }

        // ---- accessors --------------------------------------------------------------------

        public static Dictionary<string, object> Obj(object v) => v as Dictionary<string, object>;
        public static List<object> Arr(object v) => v as List<object>;

        public static object Get(object obj, string key)
        {
            var d = Obj(obj);
            return d != null && d.TryGetValue(key, out var v) ? v : null;
        }

        public static string Str(object obj, string key, string dflt = null)
        {
            var v = Get(obj, key);
            return v == null ? dflt : v is string s ? s : Serialize(v);
        }

        public static bool Bool(object obj, string key, bool dflt = false) => Get(obj, key) is bool b ? b : dflt;

        public static double Num(object obj, string key, double dflt = 0)
        {
            var v = Get(obj, key);
            return v is double d ? d : v is long l ? l : dflt;
        }

        public static long Int(object obj, string key, long dflt = 0)
        {
            var v = Get(obj, key);
            return v is long l ? l : v is double d ? (long)d : dflt;
        }

        public static List<object> List(object obj, string key) => Arr(Get(obj, key)) ?? new List<object>();

        public static List<string> Strings(object obj, string key)
        {
            var r = new List<string>();
            foreach (var x in List(obj, key)) if (x is string s) r.Add(s);
            return r;
        }

        // ---- parser -----------------------------------------------------------------------

        static void SkipWs(string s, ref int i)
        {
            while (i < s.Length && (s[i] == ' ' || s[i] == '\t' || s[i] == '\n' || s[i] == '\r')) i++;
        }

        static object ParseValue(string s, ref int i)
        {
            SkipWs(s, ref i);
            if (i >= s.Length) throw new FormatException("unexpected end of JSON");
            char c = s[i];
            switch (c)
            {
                case '{': return ParseObject(s, ref i);
                case '[': return ParseArray(s, ref i);
                case '"': return ParseString(s, ref i);
                case 't': Expect(s, ref i, "true"); return true;
                case 'f': Expect(s, ref i, "false"); return false;
                case 'n': Expect(s, ref i, "null"); return null;
                default: return ParseNumber(s, ref i);
            }
        }

        static void Expect(string s, ref int i, string word)
        {
            if (string.CompareOrdinal(s, i, word, 0, word.Length) != 0) throw new FormatException("bad JSON literal at " + i);
            i += word.Length;
        }

        static Dictionary<string, object> ParseObject(string s, ref int i)
        {
            var d = new Dictionary<string, object>();
            i++; // {
            SkipWs(s, ref i);
            if (i < s.Length && s[i] == '}') { i++; return d; }
            while (true)
            {
                SkipWs(s, ref i);
                if (i >= s.Length || s[i] != '"') throw new FormatException("expected object key at " + i);
                var k = ParseString(s, ref i);
                SkipWs(s, ref i);
                if (i >= s.Length || s[i] != ':') throw new FormatException("expected ':' at " + i);
                i++;
                d[k] = ParseValue(s, ref i);
                SkipWs(s, ref i);
                if (i >= s.Length) throw new FormatException("unterminated object");
                if (s[i] == ',') { i++; continue; }
                if (s[i] == '}') { i++; return d; }
                throw new FormatException("expected ',' or '}' at " + i);
            }
        }

        static List<object> ParseArray(string s, ref int i)
        {
            var l = new List<object>();
            i++; // [
            SkipWs(s, ref i);
            if (i < s.Length && s[i] == ']') { i++; return l; }
            while (true)
            {
                l.Add(ParseValue(s, ref i));
                SkipWs(s, ref i);
                if (i >= s.Length) throw new FormatException("unterminated array");
                if (s[i] == ',') { i++; continue; }
                if (s[i] == ']') { i++; return l; }
                throw new FormatException("expected ',' or ']' at " + i);
            }
        }

        static string ParseString(string s, ref int i)
        {
            var sb = new StringBuilder();
            i++; // "
            while (i < s.Length)
            {
                char c = s[i++];
                if (c == '"') return sb.ToString();
                if (c != '\\') { sb.Append(c); continue; }
                if (i >= s.Length) break;
                char e = s[i++];
                switch (e)
                {
                    case '"': sb.Append('"'); break;
                    case '\\': sb.Append('\\'); break;
                    case '/': sb.Append('/'); break;
                    case 'b': sb.Append('\b'); break;
                    case 'f': sb.Append('\f'); break;
                    case 'n': sb.Append('\n'); break;
                    case 'r': sb.Append('\r'); break;
                    case 't': sb.Append('\t'); break;
                    case 'u':
                        if (i + 4 > s.Length) throw new FormatException("bad \\u escape");
                        sb.Append((char)int.Parse(s.Substring(i, 4), NumberStyles.HexNumber, CultureInfo.InvariantCulture));
                        i += 4;
                        break;
                    default: throw new FormatException("bad escape \\" + e);
                }
            }
            throw new FormatException("unterminated string");
        }

        static object ParseNumber(string s, ref int i)
        {
            int start = i;
            bool frac = false;
            if (i < s.Length && s[i] == '-') i++;
            while (i < s.Length)
            {
                char c = s[i];
                if (c >= '0' && c <= '9') { i++; continue; }
                if (c == '.' || c == 'e' || c == 'E' || c == '+' || c == '-') { frac = true; i++; continue; }
                break;
            }
            var text = s.Substring(start, i - start);
            if (text.Length == 0) throw new FormatException("unexpected character at " + start);
            if (!frac && long.TryParse(text, NumberStyles.Integer, CultureInfo.InvariantCulture, out var l)) return l;
            return double.Parse(text, NumberStyles.Float, CultureInfo.InvariantCulture);
        }

        // ---- writer -----------------------------------------------------------------------

        static void Write(StringBuilder sb, object v)
        {
            switch (v)
            {
                case null: sb.Append("null"); break;
                case string s: WriteString(sb, s); break;
                case bool b: sb.Append(b ? "true" : "false"); break;
                case int n: sb.Append(n.ToString(CultureInfo.InvariantCulture)); break;
                case long n: sb.Append(n.ToString(CultureInfo.InvariantCulture)); break;
                case float n: sb.Append(n.ToString("R", CultureInfo.InvariantCulture)); break;
                case double n: sb.Append(n.ToString("R", CultureInfo.InvariantCulture)); break;
                case decimal n: sb.Append(n.ToString(CultureInfo.InvariantCulture)); break;
                case IDictionary d:
                    sb.Append('{');
                    bool first = true;
                    foreach (DictionaryEntry e in d)
                    {
                        if (!first) sb.Append(',');
                        first = false;
                        WriteString(sb, e.Key.ToString());
                        sb.Append(':');
                        Write(sb, e.Value);
                    }
                    sb.Append('}');
                    break;
                case IEnumerable list:
                    sb.Append('[');
                    bool f2 = true;
                    foreach (var x in list)
                    {
                        if (!f2) sb.Append(',');
                        f2 = false;
                        Write(sb, x);
                    }
                    sb.Append(']');
                    break;
                default: WriteString(sb, v.ToString()); break;
            }
        }

        static void WriteString(StringBuilder sb, string s)
        {
            sb.Append('"');
            foreach (char c in s)
            {
                switch (c)
                {
                    case '"': sb.Append("\\\""); break;
                    case '\\': sb.Append("\\\\"); break;
                    case '\n': sb.Append("\\n"); break;
                    case '\r': sb.Append("\\r"); break;
                    case '\t': sb.Append("\\t"); break;
                    default:
                        if (c < 0x20) sb.Append("\\u").Append(((int)c).ToString("x4"));
                        else sb.Append(c);
                        break;
                }
            }
            sb.Append('"');
        }
    }
}
