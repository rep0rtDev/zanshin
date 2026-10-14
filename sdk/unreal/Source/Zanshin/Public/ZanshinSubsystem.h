#pragma once

#include "CoreMinimal.h"
#include "Subsystems/GameInstanceSubsystem.h"
#include "ZanshinTypes.h"
#include "ZanshinSubsystem.generated.h"

class FZanshinClient;
class FZanshinStream;

DECLARE_DYNAMIC_DELEGATE_TwoParams(FZanshinSessionResult, bool, bOk, const FString&, Error);
DECLARE_DYNAMIC_MULTICAST_DELEGATE_OneParam(FZanshinFlagChanged, const FString&, Key);
DECLARE_DYNAMIC_MULTICAST_DELEGATE_TwoParams(FZanshinSessionChanged, const FString&, Token, const FString&, RefreshToken);
DECLARE_DYNAMIC_MULTICAST_DELEGATE_OneParam(FZanshinStreamState, bool, bConnected);

/**
 * Blueprint-friendly facade over FZanshinClient / FZanshinStream for the common game-side needs:
 * authenticate once, read flags synchronously from the streamed cache, track events, expose.
 * Use GetClient() from C++ for everything else.
 */
UCLASS()
class ZANSHIN_API UZanshinSubsystem : public UGameInstanceSubsystem
{
    GENERATED_BODY()

public:
    virtual void Deinitialize() override;

    /** Must be called before anything else (e.g. from your GameInstance::Init). */
    UFUNCTION(BlueprintCallable, Category = "Zanshin")
    void Configure(const FString& Url, const FString& ClientApiKey);

    /** Authenticate (or re-use persisted tokens when both are non-empty) and open the live stream. */
    UFUNCTION(BlueprintCallable, Category = "Zanshin", meta = (AutoCreateRefTerm = "DefaultProps,CustomProps"))
    void Authenticate(const FString& ExternalId, const TMap<FString, FString>& DefaultProps, const TMap<FString, FString>& CustomProps, FZanshinSessionResult OnDone);

    UFUNCTION(BlueprintCallable, Category = "Zanshin")
    void Restore(const FString& Token, const FString& RefreshToken);

    UFUNCTION(BlueprintCallable, Category = "Zanshin")
    void Logout();

    UFUNCTION(BlueprintPure, Category = "Zanshin")
    bool IsAuthenticated() const;

    /** Synchronous reads from the streamed flag cache (fallback until the first snapshot arrives). */
    UFUNCTION(BlueprintPure, Category = "Zanshin|Flags")
    bool GetFlagBool(const FString& Key, bool Fallback = false) const;
    UFUNCTION(BlueprintPure, Category = "Zanshin|Flags")
    FString GetFlagString(const FString& Key, const FString& Fallback = TEXT("")) const;
    UFUNCTION(BlueprintPure, Category = "Zanshin|Flags")
    float GetFlagNumber(const FString& Key, float Fallback = 0.f) const;
    UFUNCTION(BlueprintPure, Category = "Zanshin|Flags")
    bool HasFlag(const FString& Key) const;

    UFUNCTION(BlueprintCallable, Category = "Zanshin|Analytics", meta = (AutoCreateRefTerm = "Metadata"))
    void Track(const FString& Name, const FString& Value, const TMap<FString, FString>& Metadata);

    UFUNCTION(BlueprintCallable, Category = "Zanshin|Experiments")
    void Expose(const FString& Experiment);

    UPROPERTY(BlueprintAssignable, Category = "Zanshin")
    FZanshinFlagChanged OnFlagChanged;
    UPROPERTY(BlueprintAssignable, Category = "Zanshin")
    FZanshinSessionChanged OnSessionChanged;
    UPROPERTY(BlueprintAssignable, Category = "Zanshin")
    FZanshinStreamState OnStreamStateChanged;

    /** Full C++ API. Null until Configure() was called. */
    TSharedPtr<FZanshinClient> GetClient() const { return Client; }
    TSharedPtr<FZanshinStream> GetStream() const { return Stream; }

private:
    void OpenStream();
    TSharedPtr<FZanshinClient> Client;
    TSharedPtr<FZanshinStream> Stream;
};
