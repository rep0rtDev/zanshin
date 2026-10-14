export * from './types.js';
export { ZanshinClient } from './client.js';
export type { ClientOptions } from './client.js';
export { ZanshinStream } from './stream.js';
export type { StreamOptions, WebSocketFactory, WebSocketLike } from './stream.js';

import { ZanshinClient } from './client.js';
import { ZanshinStream, type StreamOptions } from './stream.js';

declare module './client.js' {
  interface ZanshinClient {
    /** Open a live change stream for this session. */
    stream(opts?: StreamOptions): ZanshinStream;
  }
}

ZanshinClient.prototype.stream = function (this: ZanshinClient, opts?: StreamOptions): ZanshinStream {
  return new ZanshinStream(this, opts);
};
