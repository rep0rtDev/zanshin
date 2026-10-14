import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:math';

import 'client.dart';
import 'models.dart';

/// Live change stream for one session over the `/v1/stream` WebSocket. Keeps a local cache of the
/// latest flags, pings every 30 s, reconnects with exponential backoff and re-authenticates after
/// token refresh.
///
/// ```dart
/// final stream = ZanshinStream(z);
/// stream.messages.listen((m) { if (m is FlagChanged) apply(m.flag); });
/// stream.connect();
/// ```
class ZanshinStream {
  final ZanshinClient client;
  /// Only receive changes for these flags (experiments / live events / inbox are always pushed).
  final List<String>? names;
  final bool reconnect;
  final Duration pingInterval;

  final _controller = StreamController<StreamMessage>.broadcast();
  final Map<String, Flag> _flags = {};
  WebSocket? _ws;
  Timer? _ping;
  Timer? _retry;
  bool _closed = true;
  int _attempt = 0;

  ZanshinStream(this.client, {this.names, this.reconnect = true, this.pingInterval = const Duration(seconds: 30)});

  /// Every message pushed by the server.
  Stream<StreamMessage> get messages => _controller.stream;
  /// Flag changes only.
  Stream<Flag> get flagChanges => messages.where((m) => m is FlagChanged).map((m) => (m as FlagChanged).flag);
  /// Latest known flags by key (updated from the stream).
  Map<String, Flag> get flags => Map.unmodifiable(_flags);
  bool get connected => _ws != null && _ws!.readyState == WebSocket.open;

  void connect() {
    _closed = false;
    _open();
  }

  /// Stop and do not reconnect.
  Future<void> close() async {
    _closed = true;
    _ping?.cancel();
    _retry?.cancel();
    final ws = _ws;
    _ws = null;
    await ws?.close(1000, 'bye');
    await _controller.close();
  }

  Uri _wsUrl(String token) {
    var base = client.url;
    base = base.startsWith('https') ? 'wss${base.substring(5)}' : 'ws${base.substring(4)}';
    final q = {'token': token, if (names != null && names!.isNotEmpty) 'names': names!.join(',')};
    return Uri.parse('$base/v1/stream').replace(queryParameters: q);
  }

  Future<void> _open() async {
    if (_closed) return;
    final token = client.token;
    if (token == null) {
      _scheduleReconnect();
      return;
    }
    try {
      final ws = await WebSocket.connect(_wsUrl(token).toString());
      _ws = ws;
      _attempt = 0;
      _ping = Timer.periodic(pingInterval, (_) {
        if (ws.readyState == WebSocket.open) ws.add('{"type":"ping"}');
      });
      ws.listen((data) => _dispatch(data is String ? data : utf8.decode(data as List<int>)),
          onDone: _handleClose, onError: (_) => _handleClose(), cancelOnError: true);
    } catch (_) {
      _handleClose();
    }
  }

  void _handleClose() {
    _ping?.cancel();
    _ping = null;
    _ws = null;
    _scheduleReconnect();
  }

  void _scheduleReconnect() {
    if (_closed || !reconnect) return;
    final delay = min(30000, 1000 * pow(2, min(_attempt, 5))) * (0.5 + Random().nextDouble());
    _attempt++;
    _retry = Timer(Duration(milliseconds: delay.toInt()), () async {
      if (_closed) return;
      if (client.token == null) {
        try {
          await client.refresh();
        } catch (_) {
          _scheduleReconnect();
          return;
        }
      }
      await _open();
    });
  }

  void _dispatch(String text) {
    Map<String, Object?> raw;
    try {
      raw = asMap(jsonDecode(text));
    } catch (_) {
      return;
    }
    final type = raw['type'];
    if (type is! String) return;
    late final StreamMessage msg;
    switch (type) {
      case 'hello':
        msg = HelloMessage(raw['identity_id'] is String ? raw['identity_id'] as String : '');
      case 'flag':
        final f = Flag.fromJson(asMap(raw['flag']));
        _flags[f.key] = f;
        msg = FlagChanged(f);
      case 'flag_deleted':
        final key = raw['key'] is String ? raw['key'] as String : '';
        _flags.remove(key);
        msg = FlagDeleted(key);
      case 'flags':
        final fs = asList(raw['flags']).map((f) => Flag.fromJson(asMap(f))).toList();
        for (final f in fs) {
          _flags[f.key] = f;
        }
        msg = FlagsMessage(fs);
      case 'experiments':
        msg = ExperimentsMessage(asList(raw['experiments']).map((e) => Experiment.fromJson(asMap(e))).toList());
      case 'live_events':
        msg = LiveEventsMessage(asList(raw['live_events']).map((e) => LiveEvent.fromJson(asMap(e))).toList());
      case 'inbox':
        msg = InboxChanged(asList(raw['messages']).map((m) => InboxMessage.fromJson(asMap(m))).toList());
      case 'pong':
        msg = const PongMessage();
      case 'error':
        final m = raw['message'] is String ? raw['message'] as String : '';
        if (m.toLowerCase().contains('expired')) {
          client.refresh().catchError((Object e) => throw e).ignore();
        }
        msg = StreamError(m);
      default:
        msg = UnknownMessage(type);
    }
    if (!_controller.isClosed) _controller.add(msg);
  }
}
