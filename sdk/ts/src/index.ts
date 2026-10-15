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
export { LocalEvaluator, flatten, matches } from './local.js';
export type { RulesBundle, RuleFlag, Rule, RuleAudience, Condition, FlatProps } from './local.js';
export { xxh3_64, bucket, inRollout, weightedPick } from './xxh3.js';
export { ZanshinProvider } from './openfeature.js';
export type { ZanshinProviderOptions, EvaluationContext, ResolutionDetails } from './openfeature.js';
