#include "ZanshinTypes.h"
#include "Serialization/JsonSerializer.h"
#include "Serialization/JsonWriter.h"
#include "Serialization/JsonReader.h"

namespace
{
    TSharedPtr<FJsonValue> Field(const TSharedPtr<FJsonObject>& O, const TCHAR* Name)
    {
        return O.IsValid() ? O->TryGetField(Name) : nullptr;
    }
    FString Str(const TSharedPtr<FJsonObject>& O, const TCHAR* Name)
    {
        FString S;
        if (O.IsValid()) O->TryGetStringField(Name, S);
        return S;
    }
}

bool FZanshinFlag::AsBool(bool Default) const
{
    if (!Value.IsValid() || Value->IsNull()) return Default;
    bool B;
    if (Value->TryGetBool(B)) return B;
    FString S;
    if (Value->TryGetString(S)) return S.Equals(TEXT("true"), ESearchCase::IgnoreCase);
    double D;
    if (Value->TryGetNumber(D)) return D != 0.0;
    return Default;
}

FString FZanshinFlag::AsString(const FString& Default) const
{
    if (!Value.IsValid() || Value->IsNull()) return Default;
    FString S;
    if (Value->TryGetString(S)) return S;
    double D;
    if (Value->TryGetNumber(D)) return FString::SanitizeFloat(D);
    bool B;
    if (Value->TryGetBool(B)) return B ? TEXT("true") : TEXT("false");
    return Default;
}

double FZanshinFlag::AsNumber(double Default) const
{
    if (!Value.IsValid() || Value->IsNull()) return Default;
    double D;
    if (Value->TryGetNumber(D)) return D;
    FString S;
    if (Value->TryGetString(S) && S.IsNumeric()) return FCString::Atod(*S);
    return Default;
}

TSharedPtr<FJsonObject> FZanshinFlag::AsObject() const
{
    const TSharedPtr<FJsonObject>* O = nullptr;
    if (Value.IsValid() && Value->TryGetObject(O) && O) return *O;
    return nullptr;
}

FZanshinFlag FZanshinFlag::FromJson(const TSharedPtr<FJsonObject>& O)
{
    FZanshinFlag F;
    F.Key = Str(O, TEXT("key"));
    F.Type = Str(O, TEXT("value_type"));
    F.Value = Field(O, TEXT("value"));
    F.Variant = Str(O, TEXT("variant"));
    if (O.IsValid())
    {
        const TSharedPtr<FJsonObject>* Reason = nullptr;
        if (O->TryGetObjectField(TEXT("reason"), Reason) && Reason->IsValid())
        {
            F.Reason = Str(*Reason, TEXT("kind"));
            F.Experiment = Str(*Reason, TEXT("experiment"));
        }
    }
    if (O.IsValid()) O->TryGetNumberField(TEXT("version"), F.Version);
    return F;
}

FZanshinExperiment FZanshinExperiment::FromJson(const TSharedPtr<FJsonObject>& O)
{
    FZanshinExperiment E;
    E.Key = Str(O, TEXT("key"));
    E.Variant = Str(O, TEXT("variant"));
    E.Value = Field(O, TEXT("value"));
    if (O.IsValid()) O->TryGetBoolField(TEXT("exposed"), E.bExposed);
    return E;
}

FZanshinLiveEvent FZanshinLiveEvent::FromJson(const TSharedPtr<FJsonObject>& O)
{
    FZanshinLiveEvent L;
    L.Key = Str(O, TEXT("key"));
    L.Name = Str(O, TEXT("name"));
    if (O.IsValid())
    {
        const TSharedPtr<FJsonObject>* P;
        if (O->TryGetObjectField(TEXT("payload"), P)) L.Payload = *P;
        O->TryGetBoolField(TEXT("active"), L.bActive);
        const TSharedPtr<FJsonObject>* W;
        if (O->TryGetObjectField(TEXT("current"), W) && W->IsValid())
        {
            L.bHasCurrent = true;
            L.CurrentStart = ZanshinJson::ParseDate(Str(*W, TEXT("start")));
            L.CurrentEnd = ZanshinJson::ParseDate(Str(*W, TEXT("end")));
        }
        if (O->TryGetObjectField(TEXT("next"), W) && W->IsValid())
        {
            L.bHasNext = true;
            L.NextStart = ZanshinJson::ParseDate(Str(*W, TEXT("start")));
            L.NextEnd = ZanshinJson::ParseDate(Str(*W, TEXT("end")));
        }
    }
    return L;
}

FZanshinInboxMessage FZanshinInboxMessage::FromJson(const TSharedPtr<FJsonObject>& O)
{
    FZanshinInboxMessage M;
    M.Id = Str(O, TEXT("id"));
    M.Key = Str(O, TEXT("key"));
    M.Title = Str(O, TEXT("title"));
    M.Body = Str(O, TEXT("body"));
    M.ImageUrl = Str(O, TEXT("image_url"));
    if (O.IsValid())
    {
        const TSharedPtr<FJsonObject>* P;
        if (O->TryGetObjectField(TEXT("payload"), P)) M.Payload = *P;
        O->TryGetNumberField(TEXT("priority"), M.Priority);
        M.DeliveredAt = ZanshinJson::ParseDate(Str(O, TEXT("delivered_at")));
        const FString ReadAt = Str(O, TEXT("read_at"));
        M.bRead = !ReadAt.IsEmpty();
        const FString Exp = Str(O, TEXT("expires_at"));
        M.bHasExpiry = !Exp.IsEmpty();
        if (M.bHasExpiry) M.ExpiresAt = ZanshinJson::ParseDate(Exp);
    }
    return M;
}

TSharedPtr<FJsonObject> FZanshinEvent::ToJson() const
{
    TSharedPtr<FJsonObject> O = MakeShared<FJsonObject>();
    O->SetStringField(TEXT("name"), Name);
    if (Value.IsValid() && !Value->IsNull())
    {
        // The server stores event values as strings; non-strings are sent serialized.
        FString S;
        if (Value->TryGetString(S)) O->SetStringField(TEXT("value"), S);
        else
        {
            FString Out;
            TSharedRef<TJsonWriter<TCHAR, TCondensedJsonPrintPolicy<TCHAR>>> W = TJsonWriterFactory<TCHAR, TCondensedJsonPrintPolicy<TCHAR>>::Create(&Out);
            FJsonSerializer::Serialize(Value, FString(), W);
            W->Close();
            O->SetStringField(TEXT("value"), Out);
        }
    }
    if (Metadata.IsValid()) O->SetObjectField(TEXT("metadata"), Metadata);
    O->SetStringField(TEXT("timestamp"), ZanshinJson::FormatDate(Timestamp));
    return O;
}

namespace ZanshinJson
{
    FString Serialize(const TSharedPtr<FJsonObject>& O)
    {
        FString Out;
        if (!O.IsValid()) return TEXT("{}");
        TSharedRef<TJsonWriter<TCHAR, TCondensedJsonPrintPolicy<TCHAR>>> W = TJsonWriterFactory<TCHAR, TCondensedJsonPrintPolicy<TCHAR>>::Create(&Out);
        FJsonSerializer::Serialize(O.ToSharedRef(), W);
        return Out;
    }

    TSharedPtr<FJsonObject> Parse(const FString& Text)
    {
        TSharedPtr<FJsonObject> O;
        TSharedRef<TJsonReader<>> R = TJsonReaderFactory<>::Create(Text);
        if (!FJsonSerializer::Deserialize(R, O)) return nullptr;
        return O;
    }

    FDateTime ParseDate(const FString& Iso, const FDateTime& Default)
    {
        FDateTime T;
        if (!Iso.IsEmpty() && FDateTime::ParseIso8601(*Iso, T)) return T;
        return Default;
    }

    FString FormatDate(const FDateTime& T) { return T.ToIso8601(); }

    TSharedPtr<FJsonObject> FromStringMap(const TMap<FString, FString>& M)
    {
        TSharedPtr<FJsonObject> O = MakeShared<FJsonObject>();
        for (const auto& KV : M) O->SetStringField(KV.Key, KV.Value);
        return O;
    }
}
