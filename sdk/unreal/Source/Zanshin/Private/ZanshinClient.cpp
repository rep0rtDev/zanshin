#include "ZanshinClient.h"
#include "ZanshinStream.h"
#include "ZanshinModule.h"
#include "HttpModule.h"
#include "Interfaces/IHttpResponse.h"
#include "Misc/Base64.h"
#include "GenericPlatform/GenericPlatformHttp.h"

namespace
{
    FZanshinError ErrorFrom(int32 Status, const FString& Body)
    {
        FZanshinError E;
        E.Status = Status;
        if (TSharedPtr<FJsonObject> O = ZanshinJson::Parse(Body))
        {
            O->TryGetStringField(TEXT("code"), E.Code);
            O->TryGetStringField(TEXT("message"), E.Message);
        }
        if (E.Message.IsEmpty()) E.Message = Body.IsEmpty() ? FString::Printf(TEXT("http %d"), Status) : Body.Left(200);
        if (E.Code.IsEmpty()) E.Code = Status == 0 ? TEXT("transport") : TEXT("http_error");
        return E;
    }

    template <typename T>
    TZanshinResult<T> Forward(const FZanshinError& E) { return TZanshinResult<T>::Fail(FZanshinError(E)); }

    TSharedPtr<FJsonObject> Obj(const TSharedPtr<FJsonValue>& V)
    {
        const TSharedPtr<FJsonObject>* O = nullptr;
        return (V.IsValid() && V->TryGetObject(O) && O) ? *O : nullptr;
    }

    template <typename T>
    TArray<T> ArrayOf(const TSharedPtr<FJsonValue>& V, const TCHAR* Field, T (*Make)(const TSharedPtr<FJsonObject>&))
    {
        TArray<T> Out;
        TSharedPtr<FJsonObject> O = Obj(V);
        const TArray<TSharedPtr<FJsonValue>>* Arr = nullptr;
        if (O.IsValid() && O->TryGetArrayField(Field, Arr))
            for (const auto& It : *Arr) if (TSharedPtr<FJsonObject> E = Obj(It)) Out.Add(Make(E));
        return Out;
    }
}

FZanshinClient::FZanshinClient(const FString& InUrl, const FString& InApiKey) : Url(InUrl), ApiKey(InApiKey)
{
    while (Url.EndsWith(TEXT("/"))) Url.LeftChopInline(1);
}

FZanshinClient::~FZanshinClient() {}

// ---- session ------------------------------------------------------------------------------------

void FZanshinClient::Restore(const FString& InToken, const FString& InRefreshToken)
{
    Token = InToken;
    RefreshToken = InRefreshToken;
    ExpiresAt = TokenExpiry(Token);
}

void FZanshinClient::ApplySession(const TSharedPtr<FJsonObject>& O, FZanshinSession& S)
{
    if (!O.IsValid()) return;
    O->TryGetStringField(TEXT("token"), S.Token);
    O->TryGetStringField(TEXT("refresh_token"), S.RefreshToken);
    if (const TSharedPtr<FJsonObject>* Id; O->TryGetObjectField(TEXT("identity"), Id) && Id->IsValid())
    {
        (*Id)->TryGetStringField(TEXT("id"), S.IdentityId);
        (*Id)->TryGetStringField(TEXT("external_id"), S.ExternalId);
    }
    FString Exp;
    S.ExpiresAt = O->TryGetStringField(TEXT("expires_at"), Exp) ? ZanshinJson::ParseDate(Exp) : TokenExpiry(S.Token);
    Token = S.Token;
    RefreshToken = S.RefreshToken;
    ExpiresAt = S.ExpiresAt;
    OnSessionChanged.Broadcast(S);
}

void FZanshinClient::ClearSession()
{
    Token.Empty();
    RefreshToken.Empty();
    ExpiresAt = FDateTime();
    OnSessionChanged.Broadcast(FZanshinSession());
}

FDateTime FZanshinClient::TokenExpiry(const FString& Jwt)
{
    TArray<FString> Parts;
    Jwt.ParseIntoArray(Parts, TEXT("."), true);
    if (Parts.Num() < 2) return FDateTime();
    FString B64 = Parts[1].Replace(TEXT("-"), TEXT("+")).Replace(TEXT("_"), TEXT("/"));
    while (B64.Len() % 4) B64 += TEXT("=");
    FString Payload;
    if (!FBase64::Decode(B64, Payload)) return FDateTime();
    TSharedPtr<FJsonObject> O = ZanshinJson::Parse(Payload);
    double Exp = 0;
    if (!O.IsValid() || !O->TryGetNumberField(TEXT("exp"), Exp)) return FDateTime();
    return FDateTime::FromUnixTimestamp(static_cast<int64>(Exp));
}

void FZanshinClient::Authenticate(const FString& Id, const TSharedPtr<FJsonObject>& DefaultProps, const TSharedPtr<FJsonObject>& CustomProps,
                                  TFunction<void(const TZanshinResult<FZanshinSession>&)> Done)
{
    TSharedPtr<FJsonObject> B = MakeShared<FJsonObject>();
    B->SetStringField(TEXT("id"), Id);
    if (DefaultProps.IsValid()) B->SetObjectField(TEXT("default"), DefaultProps);
    if (CustomProps.IsValid()) B->SetObjectField(TEXT("custom"), CustomProps);
    TWeakPtr<FZanshinClient> Weak = AsShared();
    Send(TEXT("POST"), TEXT("/v1/authenticate"), ZanshinJson::Serialize(B), EAuth::ApiKey, [Weak, Done](const TZanshinResult<TSharedPtr<FJsonValue>>& R) {
        TSharedPtr<FZanshinClient> Self = Weak.Pin();
        if (!R.bOk || !Self) { if (Done) Done(Forward<FZanshinSession>(R.Error)); return; }
        FZanshinSession S;
        Self->ApplySession(Obj(R.Value), S);
        if (Done) Done(TZanshinResult<FZanshinSession>::Ok(MoveTemp(S)));
    });
}

void FZanshinClient::Refresh(TFunction<void(const TZanshinResult<FZanshinSession>&)> Done)
{
    if (Done) RefreshWaiters.Add(Done);
    if (bRefreshing) return;
    if (RefreshToken.IsEmpty())
    {
        FZanshinError E; E.Status = 401; E.Code = TEXT("unauthenticated"); E.Message = TEXT("no refresh token");
        TArray<TFunction<void(const TZanshinResult<FZanshinSession>&)>> W = MoveTemp(RefreshWaiters);
        for (auto& Cb : W) Cb(TZanshinResult<FZanshinSession>::Fail(FZanshinError(E)));
        return;
    }
    bRefreshing = true;
    TSharedPtr<FJsonObject> B = MakeShared<FJsonObject>();
    B->SetStringField(TEXT("refresh_token"), RefreshToken);
    TWeakPtr<FZanshinClient> Weak = AsShared();
    Send(TEXT("POST"), TEXT("/v1/authenticate/refresh"), ZanshinJson::Serialize(B), EAuth::ApiKey, [Weak](const TZanshinResult<TSharedPtr<FJsonValue>>& R) {
        TSharedPtr<FZanshinClient> Self = Weak.Pin();
        if (!Self) return;
        Self->bRefreshing = false;
        TZanshinResult<FZanshinSession> Out;
        if (R.bOk)
        {
            FZanshinSession S;
            Self->ApplySession(Obj(R.Value), S);
            Out = TZanshinResult<FZanshinSession>::Ok(MoveTemp(S));
        }
        else
        {
            if (R.Error.Status == 401) Self->ClearSession();
            Out = Forward<FZanshinSession>(R.Error);
        }
        TArray<TFunction<void(const TZanshinResult<FZanshinSession>&)>> W = MoveTemp(Self->RefreshWaiters);
        for (auto& Cb : W) Cb(Out);
    });
}

void FZanshinClient::EnsureFresh(TFunction<void(bool)> Then)
{
    if (Token.IsEmpty()) { Then(false); return; }
    const bool bExpiring = ExpiresAt.GetTicks() != 0 && (ExpiresAt - FDateTime::UtcNow()).GetTotalSeconds() < RefreshLeewaySeconds;
    if (!bExpiring || RefreshToken.IsEmpty()) { Then(true); return; }
    Refresh([Then](const TZanshinResult<FZanshinSession>& R) { Then(R.bOk); });
}

void FZanshinClient::Logout(TFunction<void(const FZanshinVoidResult&)> Done)
{
    if (Token.IsEmpty()) { if (Done) Done(FZanshinVoidResult::Ok(FZanshinNone())); return; }
    TSharedPtr<FJsonObject> B = MakeShared<FJsonObject>();
    B->SetStringField(TEXT("token"), Token);
    B->SetStringField(TEXT("refresh_token"), RefreshToken);
    ClearSession();
    Send(TEXT("POST"), TEXT("/v1/authenticate/logout"), ZanshinJson::Serialize(B), EAuth::ApiKey, [Done](const TZanshinResult<TSharedPtr<FJsonValue>>& R) {
        if (Done) Done(R.bOk ? FZanshinVoidResult::Ok(FZanshinNone()) : Forward<FZanshinNone>(R.Error));
    });
}

// ---- transport ----------------------------------------------------------------------------------

void FZanshinClient::Request(const FString& Verb, const FString& Path, const TSharedPtr<FJsonObject>& Body, EAuth Auth, FHttpDone Done)
{
    const FString Payload = Body.IsValid() ? ZanshinJson::Serialize(Body) : FString();
    if (Auth != EAuth::Session) { Send(Verb, Path, Payload, Auth, Done); return; }
    TWeakPtr<FZanshinClient> Weak = AsShared();
    EnsureFresh([Weak, Verb, Path, Payload, Auth, Done](bool bOk) {
        TSharedPtr<FZanshinClient> Self = Weak.Pin();
        if (!Self) return;
        if (!bOk)
        {
            FZanshinError E; E.Status = 401; E.Code = TEXT("unauthenticated"); E.Message = TEXT("not authenticated");
            if (Done) Done(TZanshinResult<TSharedPtr<FJsonValue>>::Fail(MoveTemp(E)));
            return;
        }
        Self->Send(Verb, Path, Payload, Auth, Done);
    });
}

void FZanshinClient::Send(const FString& Verb, const FString& Path, const FString& Body, EAuth Auth, FHttpDone Done)
{
    TSharedRef<IHttpRequest, ESPMode::ThreadSafe> Req = FHttpModule::Get().CreateRequest();
    Req->SetURL(Url + Path);
    Req->SetVerb(Verb);
    Req->SetHeader(TEXT("Accept"), TEXT("application/json"));
    Req->SetHeader(TEXT("User-Agent"), TEXT("zanshin-unreal/0.1"));
    if (Auth == EAuth::ApiKey || (Auth == EAuth::Session && Token.IsEmpty())) Req->SetHeader(TEXT("x-api-key"), ApiKey);
    if (Auth == EAuth::Session && !Token.IsEmpty()) Req->SetHeader(TEXT("Authorization"), TEXT("Bearer ") + Token);
    if (!Body.IsEmpty())
    {
        Req->SetHeader(TEXT("Content-Type"), TEXT("application/json"));
        Req->SetContentAsString(Body);
    }
    TWeakPtr<FZanshinClient> Weak = AsShared();
    Req->OnProcessRequestComplete().BindLambda([Weak, Auth, Done](FHttpRequestPtr, FHttpResponsePtr Res, bool bConnected) {
        if (!bConnected || !Res.IsValid())
        {
            FZanshinError E; E.Status = 0; E.Code = TEXT("transport"); E.Message = TEXT("request failed");
            if (Done) Done(TZanshinResult<TSharedPtr<FJsonValue>>::Fail(MoveTemp(E)));
            return;
        }
        const int32 Status = Res->GetResponseCode();
        const FString Text = Res->GetContentAsString();
        if (Status < 200 || Status >= 300)
        {
            if (Status == 401 && Auth == EAuth::Session)
                if (TSharedPtr<FZanshinClient> Self = Weak.Pin()) Self->ClearSession();
            UE_LOG(LogZanshin, Verbose, TEXT("zanshin %d: %s"), Status, *Text);
            if (Done) Done(TZanshinResult<TSharedPtr<FJsonValue>>::Fail(ErrorFrom(Status, Text)));
            return;
        }
        TSharedPtr<FJsonValue> V;
        if (!Text.IsEmpty())
        {
            TSharedRef<TJsonReader<>> R = TJsonReaderFactory<>::Create(Text);
            FJsonSerializer::Deserialize(R, V);
        }
        if (!V.IsValid()) V = MakeShared<FJsonValueObject>(MakeShared<FJsonObject>());
        if (Done) Done(TZanshinResult<TSharedPtr<FJsonValue>>::Ok(MoveTemp(V)));
    });
    Req->ProcessRequest();
}

FString FZanshinClient::Q(const TArray<FString>& Names, const TCHAR* Extra)
{
    TArray<FString> Parts;
    if (Names.Num()) Parts.Add(TEXT("names=") + FGenericPlatformHttp::UrlEncode(FString::Join(Names, TEXT(","))));
    if (Extra) Parts.Add(Extra);
    return Parts.Num() ? TEXT("?") + FString::Join(Parts, TEXT("&")) : FString();
}

// ---- identity -----------------------------------------------------------------------------------

void FZanshinClient::GetProperties(TFunction<void(const TZanshinResult<TSharedPtr<FJsonObject>>&)> Done)
{
    Request(TEXT("GET"), TEXT("/v1/properties"), nullptr, EAuth::Session, [Done](const TZanshinResult<TSharedPtr<FJsonValue>>& R) {
        if (!Done) return;
        Done(R.bOk ? TZanshinResult<TSharedPtr<FJsonObject>>::Ok(Obj(R.Value)) : Forward<TSharedPtr<FJsonObject>>(R.Error));
    });
}

void FZanshinClient::UpdateProperties(const TSharedPtr<FJsonObject>& DefaultProps, const TSharedPtr<FJsonObject>& CustomProps,
                                      TFunction<void(const TZanshinResult<TSharedPtr<FJsonObject>>&)> Done)
{
    TSharedPtr<FJsonObject> B = MakeShared<FJsonObject>();
    if (DefaultProps.IsValid()) B->SetObjectField(TEXT("default"), DefaultProps);
    if (CustomProps.IsValid()) B->SetObjectField(TEXT("custom"), CustomProps);
    Request(TEXT("PUT"), TEXT("/v1/properties"), B, EAuth::Session, [Done](const TZanshinResult<TSharedPtr<FJsonValue>>& R) {
        if (!Done) return;
        Done(R.bOk ? TZanshinResult<TSharedPtr<FJsonObject>>::Ok(Obj(R.Value)) : Forward<TSharedPtr<FJsonObject>>(R.Error));
    });
}

void FZanshinClient::DeleteIdentity(TFunction<void(const FZanshinVoidResult&)> Done)
{
    TWeakPtr<FZanshinClient> Weak = AsShared();
    Request(TEXT("DELETE"), TEXT("/v1/identity"), nullptr, EAuth::Session, [Weak, Done](const TZanshinResult<TSharedPtr<FJsonValue>>& R) {
        if (R.bOk) if (TSharedPtr<FZanshinClient> Self = Weak.Pin()) Self->ClearSession();
        if (Done) Done(R.bOk ? FZanshinVoidResult::Ok(FZanshinNone()) : Forward<FZanshinNone>(R.Error));
    });
}

// ---- flags --------------------------------------------------------------------------------------

void FZanshinClient::GetFlags(const TArray<FString>& Names, TFunction<void(const TZanshinResult<TArray<FZanshinFlag>>&)> Done)
{
    Request(TEXT("GET"), TEXT("/v1/flags") + Q(Names), nullptr, EAuth::Session, [Done](const TZanshinResult<TSharedPtr<FJsonValue>>& R) {
        if (!Done) return;
        Done(R.bOk ? TZanshinResult<TArray<FZanshinFlag>>::Ok(ArrayOf<FZanshinFlag>(R.Value, TEXT("flags"), &FZanshinFlag::FromJson)) : Forward<TArray<FZanshinFlag>>(R.Error));
    });
}

void FZanshinClient::GetFlag(const FString& Key, TFunction<void(const TZanshinResult<FZanshinFlag>&)> Done)
{
    Request(TEXT("GET"), TEXT("/v1/flags/") + FGenericPlatformHttp::UrlEncode(Key), nullptr, EAuth::Session, [Done](const TZanshinResult<TSharedPtr<FJsonValue>>& R) {
        if (!Done) return;
        Done(R.bOk ? TZanshinResult<FZanshinFlag>::Ok(FZanshinFlag::FromJson(Obj(R.Value))) : Forward<FZanshinFlag>(R.Error));
    });
}

void FZanshinClient::FlagBool(const FString& Key, bool Fallback, TFunction<void(bool)> Done)
{
    GetFlag(Key, [Fallback, Done](const TZanshinResult<FZanshinFlag>& R) { if (Done) Done(R.bOk ? R.Value.AsBool(Fallback) : Fallback); });
}

void FZanshinClient::FlagString(const FString& Key, const FString& Fallback, TFunction<void(const FString&)> Done)
{
    GetFlag(Key, [Fallback, Done](const TZanshinResult<FZanshinFlag>& R) { if (Done) Done(R.bOk ? R.Value.AsString(Fallback) : Fallback); });
}

void FZanshinClient::FlagNumber(const FString& Key, double Fallback, TFunction<void(double)> Done)
{
    GetFlag(Key, [Fallback, Done](const TZanshinResult<FZanshinFlag>& R) { if (Done) Done(R.bOk ? R.Value.AsNumber(Fallback) : Fallback); });
}

void FZanshinClient::GetAudiences(TFunction<void(const TZanshinResult<TArray<FString>>&)> Done)
{
    Request(TEXT("GET"), TEXT("/v1/audiences"), nullptr, EAuth::Session, [Done](const TZanshinResult<TSharedPtr<FJsonValue>>& R) {
        if (!Done) return;
        if (!R.bOk) { Done(Forward<TArray<FString>>(R.Error)); return; }
        TArray<FString> Out;
        if (TSharedPtr<FJsonObject> O = Obj(R.Value)) O->TryGetStringArrayField(TEXT("audiences"), Out);
        Done(TZanshinResult<TArray<FString>>::Ok(MoveTemp(Out)));
    });
}

// ---- experiments --------------------------------------------------------------------------------

void FZanshinClient::GetExperiments(const TArray<FString>& Names, TFunction<void(const TZanshinResult<TArray<FZanshinExperiment>>&)> Done)
{
    Request(TEXT("GET"), TEXT("/v1/experiments") + Q(Names), nullptr, EAuth::Session, [Done](const TZanshinResult<TSharedPtr<FJsonValue>>& R) {
        if (!Done) return;
        Done(R.bOk ? TZanshinResult<TArray<FZanshinExperiment>>::Ok(ArrayOf<FZanshinExperiment>(R.Value, TEXT("experiments"), &FZanshinExperiment::FromJson)) : Forward<TArray<FZanshinExperiment>>(R.Error));
    });
}

void FZanshinClient::Expose(const FString& Experiment, TFunction<void(const FZanshinVoidResult&)> Done)
{
    Request(TEXT("POST"), TEXT("/v1/experiments/") + FGenericPlatformHttp::UrlEncode(Experiment) + TEXT("/expose"), nullptr, EAuth::Session, [Done](const TZanshinResult<TSharedPtr<FJsonValue>>& R) {
        if (Done) Done(R.bOk ? FZanshinVoidResult::Ok(FZanshinNone()) : Forward<FZanshinNone>(R.Error));
    });
}

// ---- live events / inbox ------------------------------------------------------------------------

void FZanshinClient::GetLiveEvents(const TArray<FString>& Names, bool bUpcoming, TFunction<void(const TZanshinResult<TArray<FZanshinLiveEvent>>&)> Done)
{
    Request(TEXT("GET"), TEXT("/v1/live-events") + Q(Names, bUpcoming ? TEXT("upcoming=true") : nullptr), nullptr, EAuth::Session, [Done](const TZanshinResult<TSharedPtr<FJsonValue>>& R) {
        if (!Done) return;
        Done(R.bOk ? TZanshinResult<TArray<FZanshinLiveEvent>>::Ok(ArrayOf<FZanshinLiveEvent>(R.Value, TEXT("live_events"), &FZanshinLiveEvent::FromJson)) : Forward<TArray<FZanshinLiveEvent>>(R.Error));
    });
}

void FZanshinClient::GetInbox(int32 Limit, TFunction<void(const TZanshinResult<TArray<FZanshinInboxMessage>>&)> Done)
{
    const FString Path = Limit > 0 ? FString::Printf(TEXT("/v1/messages?limit=%d"), Limit) : TEXT("/v1/messages");
    Request(TEXT("GET"), Path, nullptr, EAuth::Session, [Done](const TZanshinResult<TSharedPtr<FJsonValue>>& R) {
        if (!Done) return;
        Done(R.bOk ? TZanshinResult<TArray<FZanshinInboxMessage>>::Ok(ArrayOf<FZanshinInboxMessage>(R.Value, TEXT("messages"), &FZanshinInboxMessage::FromJson)) : Forward<TArray<FZanshinInboxMessage>>(R.Error));
    });
}

void FZanshinClient::MarkRead(const FString& Id, TFunction<void(const TZanshinResult<FZanshinInboxMessage>&)> Done)
{
    TSharedPtr<FJsonObject> B = MakeShared<FJsonObject>();
    B->SetBoolField(TEXT("read"), true);
    Request(TEXT("PUT"), TEXT("/v1/messages/") + FGenericPlatformHttp::UrlEncode(Id), B, EAuth::Session, [Done](const TZanshinResult<TSharedPtr<FJsonValue>>& R) {
        if (!Done) return;
        Done(R.bOk ? TZanshinResult<FZanshinInboxMessage>::Ok(FZanshinInboxMessage::FromJson(Obj(R.Value))) : Forward<FZanshinInboxMessage>(R.Error));
    });
}

void FZanshinClient::Consume(const FString& Id, TFunction<void(const FZanshinVoidResult&)> Done)
{
    Request(TEXT("DELETE"), TEXT("/v1/messages/") + FGenericPlatformHttp::UrlEncode(Id), nullptr, EAuth::Session, [Done](const TZanshinResult<TSharedPtr<FJsonValue>>& R) {
        if (Done) Done(R.bOk ? FZanshinVoidResult::Ok(FZanshinNone()) : Forward<FZanshinNone>(R.Error));
    });
}

void FZanshinClient::DeleteInboxMessage(const FString& Id, TFunction<void(const FZanshinVoidResult&)> Done)
{
    Request(TEXT("DELETE"), TEXT("/v1/inbox/") + FGenericPlatformHttp::UrlEncode(Id), nullptr, EAuth::Session, [Done](const TZanshinResult<TSharedPtr<FJsonValue>>& R) {
        if (Done) Done(R.bOk ? FZanshinVoidResult::Ok(FZanshinNone()) : Forward<FZanshinNone>(R.Error));
    });
}

void FZanshinClient::JoinLiveEvent(const FString& Key, TFunction<void(const TZanshinResult<FZanshinLiveEvent>&)> Done)
{
    Request(TEXT("POST"), TEXT("/v1/live-events/") + FGenericPlatformHttp::UrlEncode(Key) + TEXT("/join"), nullptr, EAuth::Session, [Done](const TZanshinResult<TSharedPtr<FJsonValue>>& R) {
        if (!Done) return;
        Done(R.bOk ? TZanshinResult<FZanshinLiveEvent>::Ok(FZanshinLiveEvent::FromJson(Obj(R.Value))) : Forward<FZanshinLiveEvent>(R.Error));
    });
}

void FZanshinClient::LeaveLiveEvent(const FString& Key, TFunction<void(const TZanshinResult<bool>&)> Done)
{
    Request(TEXT("POST"), TEXT("/v1/live-events/") + FGenericPlatformHttp::UrlEncode(Key) + TEXT("/leave"), nullptr, EAuth::Session, [Done](const TZanshinResult<TSharedPtr<FJsonValue>>& R) {
        if (!Done) return;
        bool bLeft = false;
        if (R.bOk && Obj(R.Value).IsValid()) Obj(R.Value)->TryGetBoolField(TEXT("left"), bLeft);
        Done(R.bOk ? TZanshinResult<bool>::Ok(bLeft) : Forward<bool>(R.Error));
    });
}

void FZanshinClient::Identify(const FString& Id, TFunction<void(const TZanshinResult<FZanshinSession>&)> Done)
{
    TSharedPtr<FJsonObject> B = MakeShared<FJsonObject>();
    B->SetStringField(TEXT("id"), Id);
    TWeakPtr<FZanshinClient> Weak = AsShared();
    Request(TEXT("POST"), TEXT("/v1/identify"), B, EAuth::Session, [Weak, Done](const TZanshinResult<TSharedPtr<FJsonValue>>& R) {
        TSharedPtr<FZanshinClient> Self = Weak.Pin();
        if (!R.bOk || !Self) { if (Done) Done(Forward<FZanshinSession>(R.Error)); return; }
        FZanshinSession S;
        Self->ApplySession(Obj(R.Value), S);
        if (Done) Done(TZanshinResult<FZanshinSession>::Ok(S));
    });
}

// ---- push / analytics ---------------------------------------------------------------------------

void FZanshinClient::RegisterPushToken(const FString& Platform, const FString& PushToken, const FString& DeviceId, TFunction<void(const FZanshinVoidResult&)> Done)
{
    TSharedPtr<FJsonObject> B = MakeShared<FJsonObject>();
    B->SetStringField(TEXT("platform"), Platform);
    B->SetStringField(TEXT("token"), PushToken);
    if (!DeviceId.IsEmpty()) B->SetStringField(TEXT("device_id"), DeviceId);
    Request(TEXT("PUT"), TEXT("/v1/push-tokens"), B, EAuth::Session, [Done](const TZanshinResult<TSharedPtr<FJsonValue>>& R) {
        if (Done) Done(R.bOk ? FZanshinVoidResult::Ok(FZanshinNone()) : Forward<FZanshinNone>(R.Error));
    });
}

void FZanshinClient::UnregisterPushToken(const FString& PushToken, TFunction<void(const FZanshinVoidResult&)> Done)
{
    Request(TEXT("DELETE"), TEXT("/v1/push-tokens/") + FGenericPlatformHttp::UrlEncode(PushToken), nullptr, EAuth::Session, [Done](const TZanshinResult<TSharedPtr<FJsonValue>>& R) {
        if (Done) Done(R.bOk ? FZanshinVoidResult::Ok(FZanshinNone()) : Forward<FZanshinNone>(R.Error));
    });
}

void FZanshinClient::Track(const TArray<FZanshinEvent>& Events, TFunction<void(const TZanshinResult<int64>&)> Done)
{
    TArray<TSharedPtr<FJsonValue>> Arr;
    for (const FZanshinEvent& E : Events) Arr.Add(MakeShared<FJsonValueObject>(E.ToJson()));
    TSharedPtr<FJsonObject> B = MakeShared<FJsonObject>();
    B->SetArrayField(TEXT("events"), Arr);
    Request(TEXT("POST"), TEXT("/v1/events"), B, EAuth::Session, [Done](const TZanshinResult<TSharedPtr<FJsonValue>>& R) {
        if (!Done) return;
        if (!R.bOk) { Done(Forward<int64>(R.Error)); return; }
        int64 N = 0;
        if (TSharedPtr<FJsonObject> O = Obj(R.Value)) O->TryGetNumberField(TEXT("accepted"), N);
        Done(TZanshinResult<int64>::Ok(MoveTemp(N)));
    });
}

void FZanshinClient::Track(const FString& Name, const TSharedPtr<FJsonValue>& Value, const TSharedPtr<FJsonObject>& Metadata)
{
    FZanshinEvent E;
    E.Name = Name;
    E.Value = Value;
    E.Metadata = Metadata;
    Track({ E }, nullptr);
}

TSharedRef<FZanshinStream> FZanshinClient::Stream(const TArray<FString>& FlagNames)
{
    return MakeShared<FZanshinStream>(AsShared(), FlagNames);
}
