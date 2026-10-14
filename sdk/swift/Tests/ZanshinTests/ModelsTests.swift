import XCTest
@testable import Zanshin

final class ModelsTests: XCTestCase {
    func testDecodesFlagAndEnvelope() throws {
        let json = #"{"flags":[{"key":"shop","value":{"price":2,"items":["a"]},"value_type":"json","reason":{"kind":"rule","rule_id":"r1"},"version":3}]}"#
        let env = try JSONDecoder().decode(Envelope<[Flag]>.self, from: Data(json.utf8))
        XCTAssertEqual(env.value.count, 1)
        XCTAssertEqual(env.value[0].value["price"]?.intValue, 2)
        XCTAssertEqual(env.value[0].reason.ruleId, "r1")
    }

    func testJwtExpiry() {
        // {"exp":4102444800}
        let jwt = "eyJhbGciOiJIUzI1NiJ9.eyJleHAiOjQxMDI0NDQ4MDB9.sig"
        XCTAssertEqual(ZanshinClient.decodeExp(jwt), 4_102_444_800)
        XCTAssertEqual(ZanshinClient.decodeExp("garbage"), 0)
    }

    func testEventEncoding() throws {
        let e = EventInput(name: "purchase", value: "9.99", metadata: ["sku": "gems_100"])
        let data = try JSONEncoder().encode(e)
        let back = try JSONDecoder().decode(EventInput.self, from: data)
        XCTAssertEqual(back, e)
    }
}
