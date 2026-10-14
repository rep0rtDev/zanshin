#include "ZanshinSubsystem.h"
#include "ZanshinClient.h"
#include "ZanshinStream.h"
#include "ZanshinModule.h"

void UZanshinSubsystem::Deinitialize()
{
    if (Stream.IsValid()) Stream->Close();
    Stream.Reset();
    Client.Reset();
    Super::Deinitialize();
}

void UZanshinSubsystem::Configure(const FString& Url, const FString& ClientApiKey)
{
    if (Stream.IsValid()) Stream->Close();
    Stream.Reset();
    Client = MakeShared<FZanshinClient>(Url, ClientApiKey);
    TWeakObjectPtr<UZanshinSubsystem> Weak(this);
    Client->OnSessionChanged.AddLambda([Weak](const FZanshinSession& S) {
        if (Weak.IsValid()) Weak->OnSessionChanged.Broadcast(S.Token, S.RefreshToken);
    });
}

void UZanshinSubsystem::Authenticate(const FString& ExternalId, const TMap<FString, FString>& DefaultProps, const TMap<FString, FString>& CustomProps, FZanshinSessionResult OnDone)
{
    if (!Client.IsValid())
    {
        UE_LOG(LogZanshin, Error, TEXT("UZanshinSubsystem::Configure must be called first"));
        OnDone.ExecuteIfBound(false, TEXT("not configured"));
        return;
    }
    TWeakObjectPtr<UZanshinSubsystem> Weak(this);
    Client->Authenticate(ExternalId, ZanshinJson::FromStringMap(DefaultProps), ZanshinJson::FromStringMap(CustomProps), [Weak, OnDone](const TZanshinResult<FZanshinSession>& R) {
        if (!Weak.IsValid()) return;
        if (R.bOk) Weak->OpenStream();
        OnDone.ExecuteIfBound(R.bOk, R.bOk ? FString() : R.Error.ToString());
    });
}

void UZanshinSubsystem::Restore(const FString& Token, const FString& RefreshToken)
{
    if (!Client.IsValid()) return;
    Client->Restore(Token, RefreshToken);
    if (Client->IsAuthenticated()) OpenStream();
}

void UZanshinSubsystem::Logout()
{
    if (Stream.IsValid()) Stream->Close();
    Stream.Reset();
    if (Client.IsValid()) Client->Logout(nullptr);
}

bool UZanshinSubsystem::IsAuthenticated() const { return Client.IsValid() && Client->IsAuthenticated(); }

void UZanshinSubsystem::OpenStream()
{
    if (!Client.IsValid()) return;
    if (Stream.IsValid()) Stream->Close();
    Stream = Client->Stream();
    TWeakObjectPtr<UZanshinSubsystem> Weak(this);
    Stream->OnFlagChanged.AddLambda([Weak](const FZanshinFlag& F) { if (Weak.IsValid()) Weak->OnFlagChanged.Broadcast(F.Key); });
    Stream->OnFlagDeleted.AddLambda([Weak](const FString& K) { if (Weak.IsValid()) Weak->OnFlagChanged.Broadcast(K); });
    Stream->OnFlagsChanged.AddLambda([Weak](const TArray<FZanshinFlag>& All) {
        if (!Weak.IsValid()) return;
        for (const FZanshinFlag& F : All) Weak->OnFlagChanged.Broadcast(F.Key);
    });
    Stream->OnConnected.AddLambda([Weak]() { if (Weak.IsValid()) Weak->OnStreamStateChanged.Broadcast(true); });
    Stream->OnDisconnected.AddLambda([Weak](const FString&) { if (Weak.IsValid()) Weak->OnStreamStateChanged.Broadcast(false); });
    Stream->Connect();
}

bool UZanshinSubsystem::HasFlag(const FString& Key) const { return Stream.IsValid() && Stream->FindFlag(Key) != nullptr; }

bool UZanshinSubsystem::GetFlagBool(const FString& Key, bool Fallback) const
{
    const FZanshinFlag* F = Stream.IsValid() ? Stream->FindFlag(Key) : nullptr;
    return F ? F->AsBool(Fallback) : Fallback;
}

FString UZanshinSubsystem::GetFlagString(const FString& Key, const FString& Fallback) const
{
    const FZanshinFlag* F = Stream.IsValid() ? Stream->FindFlag(Key) : nullptr;
    return F ? F->AsString(Fallback) : Fallback;
}

float UZanshinSubsystem::GetFlagNumber(const FString& Key, float Fallback) const
{
    const FZanshinFlag* F = Stream.IsValid() ? Stream->FindFlag(Key) : nullptr;
    return F ? static_cast<float>(F->AsNumber(Fallback)) : Fallback;
}

void UZanshinSubsystem::Track(const FString& Name, const FString& Value, const TMap<FString, FString>& Metadata)
{
    if (!Client.IsValid() || !Client->IsAuthenticated()) return;
    TSharedPtr<FJsonValue> V = Value.IsEmpty() ? nullptr : MakeShared<FJsonValueString>(Value);
    Client->Track(Name, V, Metadata.Num() ? ZanshinJson::FromStringMap(Metadata) : nullptr);
}

void UZanshinSubsystem::Expose(const FString& Experiment)
{
    if (Client.IsValid() && Client->IsAuthenticated()) Client->Expose(Experiment, nullptr);
}
