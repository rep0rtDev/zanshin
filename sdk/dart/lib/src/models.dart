/// Map of property name to JSON value.
typedef PropertyMap = Map<String, Object?>;

Map<String, Object?> _m(Object? v) => v is Map ? Map<String, Object?>.from(v) : <String, Object?>{};
List<Object?> _l(Object? v) => v is List ? List<Object?>.from(v) : <Object?>[];
String _s(Object? v, [String d = '']) => v is String ? v : d;
String? _sn(Object? v) => v is String ? v : null;
int _i(Object? v) => v is num ? v.toInt() : 0;

class Properties {
  final PropertyMap defaults;
  final PropertyMap computed;
  final PropertyMap custom;
  const Properties({required this.defaults, required this.computed, required this.custom});
  factory Properties.fromJson(Map<String, Object?> m) =>
      Properties(defaults: _m(m['default']), computed: _m(m['computed']), custom: _m(m['custom']));
}

class Identity {
  final String id;
  final String externalId;
  final String createdAt;
  final String updatedAt;
  final String lastSeenAt;
  const Identity({required this.id, required this.externalId, required this.createdAt, required this.updatedAt, required this.lastSeenAt});
  factory Identity.fromJson(Map<String, Object?> m) => Identity(
      id: _s(m['id']), externalId: _s(m['external_id']), createdAt: _s(m['created_at']),
      updatedAt: _s(m['updated_at']), lastSeenAt: _s(m['last_seen_at']));
}

class Session {
  final String token;
  final String refreshToken;
  final Identity identity;
  final Properties properties;
  const Session({required this.token, required this.refreshToken, required this.identity, required this.properties});
  factory Session.fromJson(Map<String, Object?> m) => Session(
      token: _s(m['token']), refreshToken: _s(m['refresh_token']),
      identity: Identity.fromJson(_m(m['identity'])), properties: Properties.fromJson(_m(m['properties'])));
}

/// kind: disabled | default | override | experiment | rule | variant
class Reason {
  final String kind;
  final String? ruleId;
  final String? variant;
  final String? experiment;
  const Reason({required this.kind, this.ruleId, this.variant, this.experiment});
  factory Reason.fromJson(Map<String, Object?> m) =>
      Reason(kind: _s(m['kind'], 'default'), ruleId: _sn(m['rule_id']), variant: _sn(m['variant']), experiment: _sn(m['experiment']));
}

class Flag {
  final String key;
  final Object? value;
  /// bool | string | number | json
  final String valueType;
  final String? variant;
  final Reason reason;
  final int version;
  const Flag({required this.key, required this.value, required this.valueType, this.variant, required this.reason, required this.version});
  factory Flag.fromJson(Map<String, Object?> m) => Flag(
      key: _s(m['key']), value: m['value'], valueType: _s(m['value_type'], 'json'), variant: _sn(m['variant']),
      reason: Reason.fromJson(_m(m['reason'])), version: _i(m['version']));
  bool? get boolValue => value is bool ? value as bool : null;
  String? get stringValue => value is String ? value as String : null;
  num? get numberValue => value is num ? value as num : null;
  Map<String, Object?>? get objectValue => value is Map ? _m(value) : null;
}

/// source: identity | experiment | live_event
class FlagOverride {
  final String flagKey;
  final Object? value;
  final String source;
  final String? sourceKey;
  final String? variant;
  const FlagOverride({required this.flagKey, required this.value, required this.source, this.sourceKey, this.variant});
  factory FlagOverride.fromJson(Map<String, Object?> m) => FlagOverride(
      flagKey: _s(m['flag_key']), value: m['value'], source: _s(m['source']), sourceKey: _sn(m['source_key']), variant: _sn(m['variant']));
}

class Experiment {
  final String key;
  final String name;
  final String variant;
  final Object? value;
  final int version;
  const Experiment({required this.key, required this.name, required this.variant, required this.value, required this.version});
  factory Experiment.fromJson(Map<String, Object?> m) =>
      Experiment(key: _s(m['key']), name: _s(m['name']), variant: _s(m['variant']), value: m['value'], version: _i(m['version']));
}

class Occurrence {
  final String start;
  final String end;
  const Occurrence({required this.start, required this.end});
  static Occurrence? fromJson(Object? v) => v is Map ? Occurrence(start: _s(v['start']), end: _s(v['end'])) : null;
}

class LiveEvent {
  final String key;
  final String name;
  final String description;
  final Object? value;
  final bool active;
  final Occurrence? current;
  final Occurrence? next;
  final int version;
  const LiveEvent({required this.key, required this.name, required this.description, required this.value, required this.active, this.current, this.next, required this.version});
  factory LiveEvent.fromJson(Map<String, Object?> m) => LiveEvent(
      key: _s(m['key']), name: _s(m['name']), description: _s(m['description']), value: m['value'], active: m['active'] == true,
      current: Occurrence.fromJson(m['current']), next: Occurrence.fromJson(m['next']), version: _i(m['version']));
}

class InboxMessage {
  final String id;
  final String? key;
  final String title;
  final String body;
  final String imageUrl;
  final Object? payload;
  final int priority;
  final String deliveredAt;
  final String? readAt;
  final String? expiresAt;
  const InboxMessage({required this.id, this.key, required this.title, required this.body, required this.imageUrl, required this.payload,
      required this.priority, required this.deliveredAt, this.readAt, this.expiresAt});
  factory InboxMessage.fromJson(Map<String, Object?> m) => InboxMessage(
      id: _s(m['id']), key: _sn(m['key']), title: _s(m['title']), body: _s(m['body']), imageUrl: _s(m['image_url']), payload: m['payload'],
      priority: _i(m['priority']), deliveredAt: _s(m['delivered_at']), readAt: _sn(m['read_at']), expiresAt: _sn(m['expires_at']));
}

/// platform: fcm | apns
class PushToken {
  final String identityId;
  final String platform;
  final String token;
  final String deviceId;
  final String createdAt;
  final String updatedAt;
  final int failures;
  const PushToken({required this.identityId, required this.platform, required this.token, required this.deviceId, required this.createdAt, required this.updatedAt, required this.failures});
  factory PushToken.fromJson(Map<String, Object?> m) => PushToken(
      identityId: _s(m['identity_id']), platform: _s(m['platform']), token: _s(m['token']), deviceId: _s(m['device_id']),
      createdAt: _s(m['created_at']), updatedAt: _s(m['updated_at']), failures: _i(m['failures']));
}

/// Analytics event; [timestamp] is ISO-8601 (null = server receipt time), [id] an idempotency UUID.
class EventInput {
  final String name;
  final String? value;
  final Map<String, Object?>? metadata;
  final String? timestamp;
  final String? id;
  const EventInput(this.name, {this.value, this.metadata, this.timestamp, this.id});
  Map<String, Object?> toJson() => {
        'name': name,
        if (value != null) 'value': value,
        if (metadata != null) 'metadata': metadata,
        if (timestamp != null) 'timestamp': timestamp,
        if (id != null) 'id': id,
      };
}

/// Error returned by the server or raised by the client.
class ZanshinException implements Exception {
  final int status;
  final String code;
  final String message;
  const ZanshinException(this.status, this.code, this.message);
  bool get isNotFound => status == 404;
  bool get isUnauthenticated => status == 401;
  @override
  String toString() => 'ZanshinException($code/$status: $message)';
}

/// Messages pushed on the `/v1/stream` WebSocket.
sealed class StreamMessage {
  const StreamMessage();
}

class HelloMessage extends StreamMessage {
  final String identityId;
  const HelloMessage(this.identityId);
}

class FlagChanged extends StreamMessage {
  final Flag flag;
  const FlagChanged(this.flag);
}

class FlagDeleted extends StreamMessage {
  final String key;
  const FlagDeleted(this.key);
}

class FlagsMessage extends StreamMessage {
  final List<Flag> flags;
  const FlagsMessage(this.flags);
}

class ExperimentsMessage extends StreamMessage {
  final List<Experiment> experiments;
  const ExperimentsMessage(this.experiments);
}

class LiveEventsMessage extends StreamMessage {
  final List<LiveEvent> liveEvents;
  const LiveEventsMessage(this.liveEvents);
}

class InboxChanged extends StreamMessage {
  final List<InboxMessage> messages;
  const InboxChanged(this.messages);
}

class PongMessage extends StreamMessage {
  const PongMessage();
}

class StreamError extends StreamMessage {
  final String message;
  const StreamError(this.message);
}

class UnknownMessage extends StreamMessage {
  final String type;
  const UnknownMessage(this.type);
}

// Internal helpers shared with client/stream.
Map<String, Object?> asMap(Object? v) => _m(v);
List<Object?> asList(Object? v) => _l(v);
