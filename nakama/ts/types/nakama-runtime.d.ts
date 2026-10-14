// Minimal ambient declarations for the Nakama JavaScript runtime (goja). Only the parts this
// module touches; the official `nakama-runtime` package is a drop-in superset.
declare namespace nkruntime {
  interface Logger {
    debug(format: string, ...args: unknown[]): void;
    info(format: string, ...args: unknown[]): void;
    warn(format: string, ...args: unknown[]): void;
    error(format: string, ...args: unknown[]): void;
  }
  interface Context {
    env: { [key: string]: string };
    userId?: string;
    username?: string;
    lang?: string;
    clientIp?: string;
    executionMode: string;
  }
  interface HttpResponse {
    code: number;
    headers: { [key: string]: string | string[] };
    body: string;
  }
  type RequestMethod = 'get' | 'post' | 'put' | 'patch' | 'delete' | 'head';
  interface User {
    userId: string;
    username: string;
    langTag?: string;
    location?: string;
    timezone?: string;
    createTime?: number;
  }
  interface Account {
    user: User;
  }
  interface Nakama {
    httpRequest(url: string, method: RequestMethod, headers?: { [key: string]: string }, body?: string, timeout?: number): HttpResponse;
    accountGetId(userId: string): Account;
  }
  interface Session {
    token: string;
  }
  type RpcFunction = (ctx: Context, logger: Logger, nk: Nakama, payload: string) => string | void;
  type AfterHookFunction<T, R> = (ctx: Context, logger: Logger, nk: Nakama, data: T, request: R) => void;
  interface Initializer {
    registerRpc(id: string, func: RpcFunction): void;
    registerAfterAuthenticateDevice(fn: AfterHookFunction<Session, unknown>): void;
    registerAfterAuthenticateEmail(fn: AfterHookFunction<Session, unknown>): void;
    registerAfterAuthenticateCustom(fn: AfterHookFunction<Session, unknown>): void;
    registerAfterAuthenticateGoogle(fn: AfterHookFunction<Session, unknown>): void;
    registerAfterAuthenticateApple(fn: AfterHookFunction<Session, unknown>): void;
    registerAfterAuthenticateSteam(fn: AfterHookFunction<Session, unknown>): void;
  }
  type InitModule = (ctx: Context, logger: Logger, nk: Nakama, initializer: Initializer) => void;
  const enum Codes {
    OK = 0,
    CANCELLED = 1,
    UNKNOWN = 2,
    INVALID_ARGUMENT = 3,
    DEADLINE_EXCEEDED = 4,
    NOT_FOUND = 5,
    ALREADY_EXISTS = 6,
    PERMISSION_DENIED = 7,
    RESOURCE_EXHAUSTED = 8,
    FAILED_PRECONDITION = 9,
    ABORTED = 10,
    OUT_OF_RANGE = 11,
    UNIMPLEMENTED = 12,
    INTERNAL = 13,
    UNAVAILABLE = 14,
    DATA_LOSS = 15,
    UNAUTHENTICATED = 16,
  }
  interface Error {
    message: string;
    code: Codes;
  }
}
