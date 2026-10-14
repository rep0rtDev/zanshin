#pragma once

#include "CoreMinimal.h"
#include "ZanshinTypes.h"
#include "Interfaces/IHttpRequest.h"

class FZanshinStream;

/**
 * Asynchronous client for one identity against a Zanshin server. All callbacks run on the game
 * thread (HTTP and WebSocket completion is dispatched by the engine's tickers).
 *
 *   auto Z = MakeShared<FZanshinClient>(TEXT("https://liveops.example.com"), TEXT("zk_c_..."));
 *   Z->Authenticate(PlayerId, Default, Custom, [Z](const TZanshinResult<FZanshinSession>& R) { ... });
 */
class ZANSHIN_API FZanshinClient : public TSharedFromThis<FZanshinClient>
{
public:
    FZanshinClient(const FString& Url, const FString& ApiKey);
    ~FZanshinClient();

    // ---- session ----------------------------------------------------------------------------
    FString GetToken() const { return Token; }
    FString GetRefreshToken() const { return RefreshToken; }
    bool IsAuthenticated() const { return !Token.IsEmpty(); }
    /** Seconds before access-token expiry at which calls refresh proactively (default 30). */
    int32 RefreshLeewaySeconds = 30;
    /** Fired after every rotation (and with an empty session after logout / revocation). Persist tokens here. */
    DECLARE_MULTICAST_DELEGATE_OneParam(FOnSessionChanged, const FZanshinSession&);
    FOnSessionChanged OnSessionChanged;

    /** Re-use tokens persisted from a previous run; the first call refreshes if needed. */
    void Restore(const FString& InToken, const FString& InRefreshToken);
    void Authenticate(const FString& Id, const TSharedPtr<FJsonObject>& DefaultProps, const TSharedPtr<FJsonObject>& CustomProps,
                      TFunction<void(const TZanshinResult<FZanshinSession>&)> Done);
    void Refresh(TFunction<void(const TZanshinResult<FZanshinSession>&)> Done);
    void Logout(TFunction<void(const FZanshinVoidResult&)> Done);

    // ---- identity ---------------------------------------------------------------------------
    void GetProperties(TFunction<void(const TZanshinResult<TSharedPtr<FJsonObject>>&)> Done);
    void UpdateProperties(const TSharedPtr<FJsonObject>& DefaultProps, const TSharedPtr<FJsonObject>& CustomProps,
                          TFunction<void(const TZanshinResult<TSharedPtr<FJsonObject>>&)> Done);
    void DeleteIdentity(TFunction<void(const FZanshinVoidResult&)> Done);

    // ---- flags ------------------------------------------------------------------------------
    void GetFlags(const TArray<FString>& Names, TFunction<void(const TZanshinResult<TArray<FZanshinFlag>>&)> Done);
    void GetFlag(const FString& Key, TFunction<void(const TZanshinResult<FZanshinFlag>&)> Done);
    void FlagBool(const FString& Key, bool Fallback, TFunction<void(bool)> Done);
    void FlagString(const FString& Key, const FString& Fallback, TFunction<void(const FString&)> Done);
    void FlagNumber(const FString& Key, double Fallback, TFunction<void(double)> Done);
    void GetAudiences(TFunction<void(const TZanshinResult<TArray<FString>>&)> Done);

    // ---- experiments ------------------------------------------------------------------------
    void GetExperiments(const TArray<FString>& Names, TFunction<void(const TZanshinResult<TArray<FZanshinExperiment>>&)> Done);
    void Expose(const FString& Experiment, TFunction<void(const FZanshinVoidResult&)> Done);

    // ---- live events / inbox ----------------------------------------------------------------
    void GetLiveEvents(const TArray<FString>& Names, bool bUpcoming, TFunction<void(const TZanshinResult<TArray<FZanshinLiveEvent>>&)> Done);
    void GetInbox(int32 Limit, TFunction<void(const TZanshinResult<TArray<FZanshinInboxMessage>>&)> Done);
    void MarkRead(const FString& Id, TFunction<void(const TZanshinResult<FZanshinInboxMessage>&)> Done);
    void Consume(const FString& Id, TFunction<void(const FZanshinVoidResult&)> Done);
    /** Remove a message from the inbox entirely (Consume keeps it from being redelivered). */
    void DeleteInboxMessage(const FString& Id, TFunction<void(const FZanshinVoidResult&)> Done);
    /** Explicitly join a live event with join_mode "explicit"; returns the event as now seen. */
    void JoinLiveEvent(const FString& Key, TFunction<void(const TZanshinResult<FZanshinLiveEvent>&)> Done);
    /** Leave a live event; false when the player was not a participant. */
    void LeaveLiveEvent(const FString& Key, TFunction<void(const TZanshinResult<bool>&)> Done);
    /** Link the current (typically anonymous) identity to Id, merging if it exists; switches to the new session. */
    void Identify(const FString& Id, TFunction<void(const TZanshinResult<FZanshinSession>&)> Done);

    // ---- push / analytics -------------------------------------------------------------------
    void RegisterPushToken(const FString& Platform, const FString& PushToken, const FString& DeviceId, TFunction<void(const FZanshinVoidResult&)> Done);
    void UnregisterPushToken(const FString& PushToken, TFunction<void(const FZanshinVoidResult&)> Done);
    void Track(const TArray<FZanshinEvent>& Events, TFunction<void(const TZanshinResult<int64>&)> Done);
    void Track(const FString& Name, const TSharedPtr<FJsonValue>& Value = nullptr, const TSharedPtr<FJsonObject>& Metadata = nullptr);

    /** Live WebSocket stream for this session (flag pushes, inbox, live events). */
    TSharedRef<FZanshinStream> Stream(const TArray<FString>& FlagNames = {});

    const FString& GetUrl() const { return Url; }

private:
    friend class FZanshinStream;
    enum class EAuth { None, ApiKey, Session };
    using FHttpDone = TFunction<void(const TZanshinResult<TSharedPtr<FJsonValue>>&)>;

    void Request(const FString& Verb, const FString& Path, const TSharedPtr<FJsonObject>& Body, EAuth Auth, FHttpDone Done);
    void Send(const FString& Verb, const FString& Path, const FString& Body, EAuth Auth, FHttpDone Done);
    void EnsureFresh(TFunction<void(bool)> Then);
    void ApplySession(const TSharedPtr<FJsonObject>& O, FZanshinSession& Out);
    void ClearSession();
    static FDateTime TokenExpiry(const FString& Jwt);
    static FString Q(const TArray<FString>& Names, const TCHAR* Extra = nullptr);

    FString Url;
    FString ApiKey;
    FString Token;
    FString RefreshToken;
    FDateTime ExpiresAt;
    bool bRefreshing = false;
    TArray<TFunction<void(const TZanshinResult<FZanshinSession>&)>> RefreshWaiters;
};
