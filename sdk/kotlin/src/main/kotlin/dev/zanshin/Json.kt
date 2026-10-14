package dev.zanshin

/** Tiny dependency-free JSON codec (values map to Kotlin: null, Boolean, Double, String, List, Map). */
object Json {
    fun parse(text: String): Any? = Parser(text).parseDocument()

    fun stringify(v: Any?): String = StringBuilder().also { write(it, v) }.toString()

    private fun write(sb: StringBuilder, v: Any?) {
        when (v) {
            null -> sb.append("null")
            is Boolean -> sb.append(v)
            is Int, is Long, is Short, is Byte -> sb.append(v)
            is Float, is Double -> {
                val d = (v as Number).toDouble()
                if (d == Math.floor(d) && !d.isInfinite() && Math.abs(d) < 1e15) sb.append(d.toLong()) else sb.append(d)
            }
            is Number -> sb.append(v)
            is String -> writeString(sb, v)
            is Map<*, *> -> {
                sb.append('{')
                var first = true
                for ((k, value) in v) {
                    if (!first) sb.append(',')
                    first = false
                    writeString(sb, k.toString()); sb.append(':'); write(sb, value)
                }
                sb.append('}')
            }
            is Iterable<*> -> {
                sb.append('[')
                var first = true
                for (item in v) {
                    if (!first) sb.append(',')
                    first = false
                    write(sb, item)
                }
                sb.append(']')
            }
            is Array<*> -> write(sb, v.asList())
            is JsonSerializable -> write(sb, v.toJson())
            else -> writeString(sb, v.toString())
        }
    }

    private fun writeString(sb: StringBuilder, s: String) {
        sb.append('"')
        for (c in s) {
            when (c) {
                '"' -> sb.append("\\\"")
                '\\' -> sb.append("\\\\")
                '\n' -> sb.append("\\n")
                '\r' -> sb.append("\\r")
                '\t' -> sb.append("\\t")
                '\b' -> sb.append("\\b")
                '\u000C' -> sb.append("\\f")
                else -> if (c < ' ') sb.append(String.format("\\u%04x", c.code)) else sb.append(c)
            }
        }
        sb.append('"')
    }

    private class Parser(private val s: String) {
        private var i = 0

        fun parseDocument(): Any? {
            val v = value()
            ws()
            if (i != s.length) throw IllegalArgumentException("trailing characters at $i")
            return v
        }

        private fun ws() { while (i < s.length && s[i].isWhitespace()) i++ }

        private fun value(): Any? {
            ws()
            if (i >= s.length) throw IllegalArgumentException("unexpected end of JSON")
            return when (val c = s[i]) {
                '{' -> obj()
                '[' -> arr()
                '"' -> str()
                't' -> lit("true", true)
                'f' -> lit("false", false)
                'n' -> lit("null", null)
                else -> if (c == '-' || c.isDigit()) num() else throw IllegalArgumentException("unexpected '$c' at $i")
            }
        }

        private fun lit(word: String, v: Any?): Any? {
            if (!s.startsWith(word, i)) throw IllegalArgumentException("bad literal at $i")
            i += word.length
            return v
        }

        private fun num(): Double {
            val start = i
            if (s[i] == '-') i++
            while (i < s.length && (s[i].isDigit() || s[i] in ".eE+-")) i++
            return s.substring(start, i).toDouble()
        }

        private fun str(): String {
            i++ // opening quote
            val sb = StringBuilder()
            while (true) {
                if (i >= s.length) throw IllegalArgumentException("unterminated string")
                val c = s[i++]
                when (c) {
                    '"' -> return sb.toString()
                    '\\' -> {
                        val e = s[i++]
                        when (e) {
                            '"' -> sb.append('"'); '\\' -> sb.append('\\'); '/' -> sb.append('/')
                            'b' -> sb.append('\b'); 'f' -> sb.append('\u000C'); 'n' -> sb.append('\n')
                            'r' -> sb.append('\r'); 't' -> sb.append('\t')
                            'u' -> { sb.append(s.substring(i, i + 4).toInt(16).toChar()); i += 4 }
                            else -> throw IllegalArgumentException("bad escape at $i")
                        }
                    }
                    else -> sb.append(c)
                }
            }
        }

        private fun arr(): List<Any?> {
            i++
            val out = ArrayList<Any?>()
            ws()
            if (s[i] == ']') { i++; return out }
            while (true) {
                out.add(value())
                ws()
                when (s[i++]) {
                    ',' -> continue
                    ']' -> return out
                    else -> throw IllegalArgumentException("expected , or ] at ${i - 1}")
                }
            }
        }

        private fun obj(): Map<String, Any?> {
            i++
            val out = LinkedHashMap<String, Any?>()
            ws()
            if (s[i] == '}') { i++; return out }
            while (true) {
                ws()
                val k = str()
                ws()
                if (s[i++] != ':') throw IllegalArgumentException("expected : at ${i - 1}")
                out[k] = value()
                ws()
                when (s[i++]) {
                    ',' -> continue
                    '}' -> return out
                    else -> throw IllegalArgumentException("expected , or } at ${i - 1}")
                }
            }
        }
    }
}

interface JsonSerializable { fun toJson(): Any? }

@Suppress("UNCHECKED_CAST")
internal fun Any?.asMap(): Map<String, Any?> = this as? Map<String, Any?> ?: emptyMap()
internal fun Any?.asList(): List<Any?> = this as? List<Any?> ?: emptyList()
internal fun Any?.asString(): String? = this as? String
internal fun Any?.asLong(): Long = (this as? Number)?.toLong() ?: 0L
internal fun Any?.asInt(): Int = (this as? Number)?.toInt() ?: 0
internal fun Any?.asBool(): Boolean = this as? Boolean ?: false
