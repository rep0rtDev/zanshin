#pragma once

#include "CoreMinimal.h"
#include "Dom/JsonObject.h"
#include "Dom/JsonValue.h"

/** Error returned by the server or the transport. Status 0 = network / parse failure. */
struct ZANSHIN_API FZanshinError
{
    int32 Status = 0;
    FString Code;
    FString Message;

    bool IsNotFound() const { return Status == 404; }
    bool IsUnauthenticated() const { return Status == 401; }
    bool IsRateLimited() const { return Status == 429; }
    FString ToString() const { return FString::Printf(TEXT("%d %s: %s"), Status, *Code, *Message); }
};

/** Result of an asynchronous call: either a value or an error. */
template <typename T>
struct TZanshinResult
{
    bool bOk = false;
    T Value{};
    FZanshinError Error;

    static TZanshinResult Ok(T&& V) { TZanshinResult R; R.bOk = true; R.Value = MoveTemp(V); return R; }
    static TZanshinResult Fail(FZanshinError&& E) { TZanshinResult R; R.bOk = false; R.Error = MoveTemp(E); return R; }
};

struct FZanshinNone {};
using FZanshinVoidResult = TZanshinResult<FZanshinNone>;

struct ZANSHIN_API FZanshinSession
{
    FString Token;
    FString RefreshToken;
    FString IdentityId;
    FString ExternalId;
    FDateTime ExpiresAt;
};

struct ZANSHIN_API FZanshinFlag
{
    FString Key;
    FString Type;          // bool | string | number | json
    TSharedPtr<FJsonValue> Value;
    FString Variant;
    FString Reason;        // default | rule | override | experiment | disabled ...
    FString Experiment;    // set when Reason == "experiment"
    int64 Version = 0;

    bool AsBool(bool Default = false) const;
    FString AsString(const FString& Default = FString()) const;
    double AsNumber(double Default = 0.0) const;
    TSharedPtr<FJsonObject> AsObject() const;

    static FZanshinFlag FromJson(const TSharedPtr<FJsonObject>& O);
};

struct ZANSHIN_API FZanshinExperiment
{
    FString Key;
    FString Variant;
    TSharedPtr<FJsonValue> Value;
    bool bExposed = false;

    static FZanshinExperiment FromJson(const TSharedPtr<FJsonObject>& O);
};

struct ZANSHIN_API FZanshinLiveEvent
{
    FString Key;
    FString Name;
    TSharedPtr<FJsonObject> Payload;
    bool bActive = false;
    FDateTime CurrentStart, CurrentEnd, NextStart, NextEnd;
    bool bHasCurrent = false, bHasNext = false;

    static FZanshinLiveEvent FromJson(const TSharedPtr<FJsonObject>& O);
};

struct ZANSHIN_API FZanshinInboxMessage
{
    FString Id;
    FString Key;
    FString Title;
    FString Body;
    FString ImageUrl;
    TSharedPtr<FJsonObject> Payload;
    int32 Priority = 0;
    FDateTime DeliveredAt;
    bool bRead = false;
    bool bHasExpiry = false;
    FDateTime ExpiresAt;

    static FZanshinInboxMessage FromJson(const TSharedPtr<FJsonObject>& O);
};

struct ZANSHIN_API FZanshinEvent
{
    FString Name;
    TSharedPtr<FJsonValue> Value;
    TSharedPtr<FJsonObject> Metadata;
    FDateTime Timestamp = FDateTime::UtcNow();

    TSharedPtr<FJsonObject> ToJson() const;
};

namespace ZanshinJson
{
    ZANSHIN_API FString Serialize(const TSharedPtr<FJsonObject>& O);
    ZANSHIN_API TSharedPtr<FJsonObject> Parse(const FString& Text);
    ZANSHIN_API FDateTime ParseDate(const FString& Iso, const FDateTime& Default = FDateTime());
    ZANSHIN_API FString FormatDate(const FDateTime& T);
    ZANSHIN_API TSharedPtr<FJsonObject> FromStringMap(const TMap<FString, FString>& M);
}
