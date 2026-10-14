import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'models.dart';

/// Zanshin client for a single identity (one player / device).
///
/// ```dart
/// final z = ZanshinClient(url: 'https://liveops.example.com', apiKey: 'zk_c_...');
/// await z.authenticate(deviceId, defaults: {'platform': 'android'});
/// final shop = (await z.flag('shop_v2')).boolValue ?? false;
/// ```
class ZanshinClient {
  /// REST base URL (no trailing slash).
  final String url;
  final String _apiKey;
  final HttpClient _http;
  final Duration _leeway;
  /// Called whenever tokens change (persist them to resume the session later); null on logout.
  final void Function(({String token, String refreshToken})? tokens)? onSession;

  String? _token;
  String? _refreshToken;
  int _expiresAt = 0;
  Future<Session>? _refreshing;

  ZanshinClient({
    required String url,
    required String apiKey,
    HttpClient? httpClient,
    Duration refreshLeeway = const Duration(seconds: 60),
    this.onSession,
  })  : url = url.replaceAll(RegExp(r'/+$'), ''),
        _apiKey = apiKey,
        _http = httpClient ?? HttpClient(),
        _leeway = refreshLeeway;

  /// Current access token, or null when not authenticated.
  String? get token => _token;
  bool get isAuthenticated => _token != null;

  // ---- session ------------------------------------------------------------------------------

  /// Resume a persisted session; refreshes immediately if the access token is stale.
  Future<void> restore(String token, String refreshToken) async {
    _setTokens(token, refreshToken, notify: false);
    if (_expiringSoon()) await refresh();
  }

  /// Authenticate (create or resume) the identity [id] and open a session.
  Future<Session> authenticate(String id, {PropertyMap defaults = const {}, PropertyMap custom = const {}}) async {
    final s = Session.fromJson(asMap(await _request('POST', '/v1/authenticate',
        auth: _Auth.key, body: {'id': id, 'default': defaults, 'custom': custom})));
    _setTokens(s.token, s.refreshToken);
    return s;
  }

  /// Rotate tokens. Concurrent calls share one request.
  Future<Session> refresh() {
    final pending = _refreshing;
    if (pending != null) return pending;
    final rt = _refreshToken;
    if (rt == null || rt.isEmpty) {
      return Future.error(const ZanshinException(401, 'unauthenticated', 'no refresh token'));
    }
    final f = () async {
      try {
        final s = Session.fromJson(asMap(await _request('POST', '/v1/authenticate/refresh', auth: _Auth.key, body: {'refresh_token': rt})));
        _setTokens(s.token, s.refreshToken);
        return s;
      } on ZanshinException catch (e) {
        if (e.status == 401) _clearTokens();
        rethrow;
      } finally {
        _refreshing = null;
      }
    }();
    _refreshing = f;
    return f;
  }

  /// Revoke the session server-side and forget tokens.
  Future<void> logout() async {
    final t = _token;
    final rt = _refreshToken;
    if (t == null) return;
    _clearTokens();
    await _request('POST', '/v1/authenticate/logout', auth: _Auth.key, body: {'token': t, 'refresh_token': rt});
  }

  // ---- identity & properties ----------------------------------------------------------------

  Future<Identity> identity() async => Identity.fromJson(asMap(await _request('GET', '/v1/identity')));

  /// Delete the identity and everything attached to it.
  Future<void> deleteIdentity() async {
    await _request('DELETE', '/v1/identity');
    _clearTokens();
  }

  Future<Properties> properties() async => Properties.fromJson(asMap(await _request('GET', '/v1/properties')));

  /// Merge properties; a null value removes the key.
  Future<Properties> updateProperties({PropertyMap defaults = const {}, PropertyMap custom = const {}}) async =>
      Properties.fromJson(asMap(await _request('PUT', '/v1/properties', body: {'default': defaults, 'custom': custom})));

  /// Link the current (typically anonymous) identity to [id]; the client switches to the new session.
  Future<Session> identify(String id) async {
    final s = Session.fromJson(asMap(await _request('POST', '/v1/identify', body: {'id': id})));
    _setTokens(s.token, s.refreshToken);
    return s;
  }

  // ---- flags --------------------------------------------------------------------------------

  Future<List<Flag>> flags({List<String>? names}) async =>
      asList(asMap(await _request('GET', '/v1/flags', query: {'names': names?.join(',')}))['flags']).map((f) => Flag.fromJson(asMap(f))).toList();

  Future<Flag> flag(String key) async => Flag.fromJson(asMap(await _request('GET', '/v1/flags/${Uri.encodeComponent(key)}')));

  /// Flag value with a fallback when the flag does not exist.
  Future<T> flagValue<T>(String key, T fallback) async {
    try {
      final v = (await flag(key)).value;
      return v is T ? v : fallback;
    } on ZanshinException catch (e) {
      if (e.isNotFound) return fallback;
      rethrow;
    }
  }

  /// Every identity / experiment / live-event override applied to the caller right now.
  Future<List<FlagOverride>> flagOverrides() async =>
      asList(asMap(await _request('GET', '/v1/flags/overrides'))['overrides']).map((o) => FlagOverride.fromJson(asMap(o))).toList();

  Future<List<String>> audiences() async => asList(asMap(await _request('GET', '/v1/audiences'))['audiences']).whereType<String>().toList();

  // ---- experiments --------------------------------------------------------------------------

  Future<List<Experiment>> experiments({List<String>? names}) async =>
      asList(asMap(await _request('GET', '/v1/experiments', query: {'names': names?.join(',')}))['experiments'])
          .map((e) => Experiment.fromJson(asMap(e)))
          .toList();

  /// Record that the identity actually saw its variant.
  Future<void> expose(String experiment) => _request('POST', '/v1/experiments/${Uri.encodeComponent(experiment)}/expose');

  // ---- live events --------------------------------------------------------------------------

  Future<List<LiveEvent>> liveEvents({List<String>? names, bool upcoming = false}) async =>
      asList(asMap(await _request('GET', '/v1/live-events', query: {'names': names?.join(','), 'upcoming': upcoming ? 'true' : null}))['live_events'])
          .map((e) => LiveEvent.fromJson(asMap(e)))
          .toList();

  /// Explicitly join a live event with `join_mode: explicit`; returns the event as now seen.
  Future<LiveEvent> joinLiveEvent(String key) async =>
      LiveEvent.fromJson(asMap(await _request('POST', '/v1/live-events/${Uri.encodeComponent(key)}/join')));

  /// Leave a live event; false when the identity was not a participant.
  Future<bool> leaveLiveEvent(String key) async =>
      asMap(await _request('POST', '/v1/live-events/${Uri.encodeComponent(key)}/leave'))['left'] == true;

  // ---- inbox --------------------------------------------------------------------------------

  Future<List<InboxMessage>> inbox({int? limit}) async =>
      asList(asMap(await _request('GET', '/v1/messages', query: {'limit': limit?.toString()}))['messages']).map((m) => InboxMessage.fromJson(asMap(m))).toList();

  Future<InboxMessage> markRead(String id) async =>
      InboxMessage.fromJson(asMap(await _request('PUT', '/v1/messages/${Uri.encodeComponent(id)}', body: {'read': true})));

  /// Consume (remove) a message; it is never redelivered.
  Future<void> consume(String id) => _request('DELETE', '/v1/messages/${Uri.encodeComponent(id)}');

  /// Remove a message from the inbox entirely.
  Future<void> deleteInboxMessage(String id) => _request('DELETE', '/v1/inbox/${Uri.encodeComponent(id)}');

  // ---- push tokens --------------------------------------------------------------------------

  /// Register this device for push notifications (`fcm` registration token or `apns` hex token).
  Future<PushToken> registerPushToken(String platform, String token, {String deviceId = ''}) async =>
      PushToken.fromJson(asMap(await _request('PUT', '/v1/push-tokens', body: {'platform': platform, 'token': token, 'device_id': deviceId})));

  Future<List<PushToken>> pushTokens() async =>
      asList(asMap(await _request('GET', '/v1/push-tokens'))['tokens']).map((t) => PushToken.fromJson(asMap(t))).toList();

  Future<void> unregisterPushToken(String token) => _request('DELETE', '/v1/push-tokens/${Uri.encodeComponent(token)}');

  // ---- analytics ----------------------------------------------------------------------------

  /// Report events; returns how many were accepted.
  Future<int> track(List<EventInput> events) async {
    if (events.isEmpty) return 0;
    final r = asMap(await _request('POST', '/v1/events', body: {'events': events.map((e) => e.toJson()).toList()}));
    return r['accepted'] is num ? (r['accepted'] as num).toInt() : 0;
  }

  /// Convenience: one event.
  Future<int> event(String name, {Object? value, Map<String, Object?>? metadata}) =>
      track([EventInput(name, value: value?.toString(), metadata: metadata)]);

  // ---- transport ----------------------------------------------------------------------------

  void _setTokens(String token, String refreshToken, {bool notify = true}) {
    _token = token;
    _refreshToken = refreshToken;
    _expiresAt = decodeExp(token);
    if (notify) onSession?.call((token: token, refreshToken: refreshToken));
  }

  void _clearTokens() {
    _token = null;
    _refreshToken = null;
    _expiresAt = 0;
    onSession?.call(null);
  }

  bool _expiringSoon() {
    if (_token == null) return true;
    if (_expiresAt == 0) return false;
    return DateTime.now().millisecondsSinceEpoch ~/ 1000 >= _expiresAt - _leeway.inSeconds;
  }

  Future<Object?> _request(String method, String path, {_Auth auth = _Auth.session, Map<String, String?> query = const {}, Object? body}) async {
    final q = <String, String>{for (final e in query.entries) if (e.value != null && e.value!.isNotEmpty) e.key: e.value!};
    final uri = Uri.parse(url + path).replace(queryParameters: q.isEmpty ? null : q);
    final req = await _http.openUrl(method, uri);
    req.headers.set('accept', 'application/json');
    switch (auth) {
      case _Auth.key:
        req.headers.set('x-api-key', _apiKey);
      case _Auth.session:
        if (_token == null) throw const ZanshinException(401, 'unauthenticated', 'not authenticated');
        if (_expiringSoon() && (_refreshToken ?? '').isNotEmpty) await refresh();
        req.headers.set('authorization', 'Bearer $_token');
      case _Auth.none:
        break;
    }
    if (body != null) {
      req.headers.set('content-type', 'application/json');
      req.write(jsonEncode(body));
    }
    final res = await req.close();
    final text = await utf8.decodeStream(res);
    if (res.statusCode < 200 || res.statusCode >= 300) {
      var code = 'error';
      var message = text;
      try {
        final e = asMap(jsonDecode(text));
        if (e['code'] is String) code = e['code'] as String;
        if (e['message'] is String) message = e['message'] as String;
      } catch (_) {/* non-JSON error body */}
      if (res.statusCode == 401 && auth == _Auth.session) _clearTokens();
      throw ZanshinException(res.statusCode, code, message);
    }
    return text.isEmpty ? <String, Object?>{} : jsonDecode(text);
  }

  /// `exp` claim of a JWT (unix seconds), 0 when absent.
  static int decodeExp(String jwt) {
    try {
      final parts = jwt.split('.');
      if (parts.length < 2) return 0;
      final payload = utf8.decode(base64Url.decode(base64Url.normalize(parts[1])));
      final exp = asMap(jsonDecode(payload))['exp'];
      return exp is num ? exp.toInt() : 0;
    } catch (_) {
      return 0;
    }
  }
}

enum _Auth { key, session, none }
