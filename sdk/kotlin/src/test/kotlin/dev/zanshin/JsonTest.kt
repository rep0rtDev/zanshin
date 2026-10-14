package dev.zanshin

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull

class JsonTest {
    @Test
    fun roundTrip() {
        val src = """{"flags":[{"key":"shop","value":{"price":2,"items":["a","b"]},"value_type":"json","reason":{"kind":"rule","rule_id":"r1"},"version":3,"variant":null}]}"""
        val parsed = Json.parse(src).asMap()
        val flag = Flag.from(parsed["flags"].asList()[0].asMap())
        assertEquals("shop", flag.key)
        assertEquals(2.0, flag.objectValue?.get("price"))
        assertEquals("r1", flag.reason.ruleId)
        assertNull(flag.variant)
        assertEquals(src.replace("\"variant\":null", "\"variant\":null"), Json.stringify(parsed))
    }

    @Test
    fun escapes() {
        val s = "quote\" back\\ nl\n tab\t uni\u0001"
        assertEquals(s, Json.parse(Json.stringify(s)))
    }

    @Test
    fun jwtExpiry() {
        assertEquals(4102444800L, ZanshinClient.decodeExp("eyJhbGciOiJIUzI1NiJ9.eyJleHAiOjQxMDI0NDQ4MDB9.sig"))
        assertEquals(0L, ZanshinClient.decodeExp("garbage"))
    }

    @Test
    fun eventSerialization() {
        val e = EventInput("purchase", "9.99", mapOf("sku" to "gems_100"))
        assertEquals("""{"name":"purchase","value":"9.99","metadata":{"sku":"gems_100"}}""", Json.stringify(e))
    }
}
