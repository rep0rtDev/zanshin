import 'dart:convert';

import 'package:test/test.dart';
import 'package:zanshin/zanshin.dart';

void main() {
  test('decodes flags', () {
    const json = '{"flags":[{"key":"shop","value":{"price":2,"items":["a"]},"value_type":"json","reason":{"kind":"rule","rule_id":"r1"},"version":3}]}';
    final flags = asList(asMap(jsonDecode(json))['flags']).map((f) => Flag.fromJson(asMap(f))).toList();
    expect(flags.single.key, 'shop');
    expect(flags.single.objectValue?['price'], 2);
    expect(flags.single.reason.ruleId, 'r1');
    expect(flags.single.boolValue, isNull);
  });

  test('jwt expiry', () {
    expect(ZanshinClient.decodeExp('eyJhbGciOiJIUzI1NiJ9.eyJleHAiOjQxMDI0NDQ4MDB9.sig'), 4102444800);
    expect(ZanshinClient.decodeExp('garbage'), 0);
  });

  test('event json', () {
    const e = EventInput('purchase', value: '9.99', metadata: {'sku': 'gems_100'});
    expect(jsonEncode(e.toJson()), '{"name":"purchase","value":"9.99","metadata":{"sku":"gems_100"}}');
  });

  test('live event occurrences', () {
    final e = LiveEvent.fromJson({'key': 'raid', 'active': true, 'current': {'start': 'a', 'end': 'b'}, 'version': 1});
    expect(e.current?.start, 'a');
    expect(e.next, isNull);
  });
}
