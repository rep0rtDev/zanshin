#include "ZanshinStream.h"
#include "ZanshinClient.h"
#include "ZanshinModule.h"
#include "IWebSocket.h"
#include "WebSocketsModule.h"
#include "GenericPlatform/GenericPlatformHttp.h"

namespace
{
    template <typename T>
    TArray<T> Items(const TSharedPtr<FJsonObject>& O, const TCHAR* Field, T (*Make)(const TSharedPtr<FJsonObject>&))
    {
        TArray<T> Out;
        const TArray<TSharedPtr<FJsonValue>>* Arr = nullptr;
        if (O.IsValid() && O->TryGetArrayField(Field, Arr))
            for (const auto& It : *Arr)
            {
                const TSharedPtr<FJsonObject>* E = nullptr;
                if (It.IsValid() && It->TryGetObject(E) && E) Out.Add(Make(*E));
            }
        return Out;
    }
}

FZanshinStream::FZanshinStream(TSharedRef<FZanshinClient> InClient, const TArray<FString>& InFlagNames)
    : Client(InClient), FlagNames(InFlagNames) {}

FZanshinStream::~FZanshinStream()
{
    bClosed = true;
    StopTimers();
    if (Socket.IsValid())
    {
        Socket->OnMessage().Clear();
        Socket->OnClosed().Clear();
        Socket->OnConnected().Clear();
        Socket->OnConnectionError().Clear();
        if (Socket->IsConnected()) Socket->Close();
    }
}

FString FZanshinStream::BuildUrl() const
{
    FString Base = Client->GetUrl();
    if (Base.StartsWith(TEXT("https://"))) Base = TEXT("wss://") + Base.Mid(8);
    else if (Base.StartsWith(TEXT("http://"))) Base = TEXT("ws://") + Base.Mid(7);
    FString Q = TEXT("token=") + FGenericPlatformHttp::UrlEncode(Client->GetToken());
    if (FlagNames.Num()) Q += TEXT("&names=") + FGenericPlatformHttp::UrlEncode(FString::Join(FlagNames, TEXT(",")));
    return Base + TEXT("/v1/stream?") + Q;
}

void FZanshinStream::Connect()
{
    bClosed = false;
    Attempt = 0;
    Open();
}

void FZanshinStream::Close()
{
    bClosed = true;
    StopTimers();
    if (Socket.IsValid() && Socket->IsConnected()) Socket->Close();
    bConnected = false;
}

void FZanshinStream::StopTimers()
{
    if (PingHandle.IsValid()) { FTSTicker::GetCoreTicker().RemoveTicker(PingHandle); PingHandle.Reset(); }
    if (ReconnectHandle.IsValid()) { FTSTicker::GetCoreTicker().RemoveTicker(ReconnectHandle); ReconnectHandle.Reset(); }
}

void FZanshinStream::Open()
{
    if (bClosed) return;
    if (!Client->IsAuthenticated())
    {
        FZanshinError E; E.Status = 401; E.Code = TEXT("unauthenticated"); E.Message = TEXT("stream needs a session");
        OnError.Broadcast(E);
        ScheduleReconnect();
        return;
    }
    Socket = FWebSocketsModule::Get().CreateWebSocket(BuildUrl(), TEXT(""));
    TWeakPtr<FZanshinStream> Weak = AsShared();
    Socket->OnConnected().AddLambda([Weak]() {
        TSharedPtr<FZanshinStream> Self = Weak.Pin();
        if (!Self) return;
        Self->bConnected = true;
        Self->Attempt = 0;
        Self->OnConnected.Broadcast();
        Self->PingHandle = FTSTicker::GetCoreTicker().AddTicker(FTickerDelegate::CreateLambda([Weak](float) {
            TSharedPtr<FZanshinStream> S = Weak.Pin();
            if (S && S->Socket.IsValid() && S->Socket->IsConnected()) S->Socket->Send(TEXT("{\"type\":\"ping\"}"));
            return true;
        }), 30.0f);
    });
    Socket->OnConnectionError().AddLambda([Weak](const FString& Err) {
        TSharedPtr<FZanshinStream> Self = Weak.Pin();
        if (!Self) return;
        FZanshinError E; E.Status = 0; E.Code = TEXT("transport"); E.Message = Err;
        Self->OnError.Broadcast(E);
        Self->ScheduleReconnect();
    });
    Socket->OnClosed().AddLambda([Weak](int32 Code, const FString& Reason, bool) {
        TSharedPtr<FZanshinStream> Self = Weak.Pin();
        if (!Self) return;
        Self->bConnected = false;
        if (Self->PingHandle.IsValid()) { FTSTicker::GetCoreTicker().RemoveTicker(Self->PingHandle); Self->PingHandle.Reset(); }
        Self->OnDisconnected.Broadcast(FString::Printf(TEXT("%d %s"), Code, *Reason));
        // 4401 = session expired/invalid: refresh before reconnecting.
        if (Code == 4401 || Code == 1008)
        {
            TWeakPtr<FZanshinStream> W2 = Weak;
            Self->Client->Refresh([W2](const TZanshinResult<FZanshinSession>&) { if (TSharedPtr<FZanshinStream> S = W2.Pin()) S->ScheduleReconnect(); });
            return;
        }
        Self->ScheduleReconnect();
    });
    Socket->OnMessage().AddLambda([Weak](const FString& Text) {
        if (TSharedPtr<FZanshinStream> Self = Weak.Pin()) Self->HandleMessage(Text);
    });
    Socket->Connect();
}

void FZanshinStream::ScheduleReconnect()
{
    if (bClosed || ReconnectHandle.IsValid()) return;
    const float Base = FMath::Min(30.0f, 0.5f * FMath::Pow(2.0f, static_cast<float>(FMath::Min(Attempt, 6))));
    const float Delay = Base + FMath::FRandRange(0.0f, Base * 0.3f);
    Attempt++;
    TWeakPtr<FZanshinStream> Weak = AsShared();
    ReconnectHandle = FTSTicker::GetCoreTicker().AddTicker(FTickerDelegate::CreateLambda([Weak](float) {
        if (TSharedPtr<FZanshinStream> Self = Weak.Pin())
        {
            Self->ReconnectHandle.Reset();
            Self->Open();
        }
        return false;
    }), Delay);
}

void FZanshinStream::HandleMessage(const FString& Text)
{
    TSharedPtr<FJsonObject> M = ZanshinJson::Parse(Text);
    if (!M.IsValid()) return;
    FString Type;
    M->TryGetStringField(TEXT("type"), Type);
    if (Type == TEXT("flag"))
    {
        const TSharedPtr<FJsonObject>* F = nullptr;
        if (M->TryGetObjectField(TEXT("flag"), F) && F->IsValid())
        {
            FZanshinFlag Flag = FZanshinFlag::FromJson(*F);
            Flags.Add(Flag.Key, Flag);
            OnFlagChanged.Broadcast(Flag);
        }
    }
    else if (Type == TEXT("flag_deleted"))
    {
        FString Key;
        M->TryGetStringField(TEXT("key"), Key);
        Flags.Remove(Key);
        OnFlagDeleted.Broadcast(Key);
    }
    else if (Type == TEXT("flags"))
    {
        TArray<FZanshinFlag> All = Items<FZanshinFlag>(M, TEXT("flags"), &FZanshinFlag::FromJson);
        for (const FZanshinFlag& F : All) Flags.Add(F.Key, F);
        OnFlagsChanged.Broadcast(All);
    }
    else if (Type == TEXT("experiments")) OnExperimentsChanged.Broadcast(Items<FZanshinExperiment>(M, TEXT("experiments"), &FZanshinExperiment::FromJson));
    else if (Type == TEXT("live_events")) OnLiveEventsChanged.Broadcast(Items<FZanshinLiveEvent>(M, TEXT("live_events"), &FZanshinLiveEvent::FromJson));
    else if (Type == TEXT("inbox")) OnInboxChanged.Broadcast(Items<FZanshinInboxMessage>(M, TEXT("messages"), &FZanshinInboxMessage::FromJson));
    else if (Type == TEXT("error"))
    {
        FZanshinError E;
        M->TryGetStringField(TEXT("code"), E.Code);
        M->TryGetStringField(TEXT("message"), E.Message);
        OnError.Broadcast(E);
        if (E.Message.Contains(TEXT("expired"), ESearchCase::IgnoreCase))
        {
            // The server closes right after; refresh so the reconnect carries a valid token.
            TWeakPtr<FZanshinStream> Weak = AsShared();
            Client->Refresh([Weak](const TZanshinResult<FZanshinSession>&) {});
        }
    }
    // "hello" and "pong" need no handling.
}
