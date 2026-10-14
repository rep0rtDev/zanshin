#pragma once

#include "CoreMinimal.h"
#include "ZanshinTypes.h"
#include "Containers/Ticker.h"

class FZanshinClient;
class IWebSocket;

/**
 * Live change stream over `/v1/stream`: reconnects with jittered exponential backoff, pings every
 * 30 s, keeps a local cache of streamed flags and refreshes the session when the server reports it
 * expired. All delegates fire on the game thread.
 */
class ZANSHIN_API FZanshinStream : public TSharedFromThis<FZanshinStream>
{
public:
    FZanshinStream(TSharedRef<FZanshinClient> InClient, const TArray<FString>& InFlagNames);
    ~FZanshinStream();

    void Connect();
    void Close();
    bool IsConnected() const { return bConnected; }

    /** Flags received over the stream (initial snapshot + pushes), by key. */
    const TMap<FString, FZanshinFlag>& GetFlags() const { return Flags; }
    const FZanshinFlag* FindFlag(const FString& Key) const { return Flags.Find(Key); }

    DECLARE_MULTICAST_DELEGATE(FOnConnected);
    DECLARE_MULTICAST_DELEGATE_OneParam(FOnDisconnected, const FString& /*Reason*/);
    DECLARE_MULTICAST_DELEGATE_OneParam(FOnError, const FZanshinError&);
    DECLARE_MULTICAST_DELEGATE_OneParam(FOnFlag, const FZanshinFlag&);
    DECLARE_MULTICAST_DELEGATE_OneParam(FOnFlagDeleted, const FString& /*Key*/);
    DECLARE_MULTICAST_DELEGATE_OneParam(FOnFlags, const TArray<FZanshinFlag>&);
    DECLARE_MULTICAST_DELEGATE_OneParam(FOnExperiments, const TArray<FZanshinExperiment>&);
    DECLARE_MULTICAST_DELEGATE_OneParam(FOnLiveEvents, const TArray<FZanshinLiveEvent>&);
    DECLARE_MULTICAST_DELEGATE_OneParam(FOnInbox, const TArray<FZanshinInboxMessage>&);

    FOnConnected OnConnected;
    FOnDisconnected OnDisconnected;
    FOnError OnError;
    FOnFlag OnFlagChanged;
    FOnFlagDeleted OnFlagDeleted;
    FOnFlags OnFlagsChanged;
    FOnExperiments OnExperimentsChanged;
    FOnLiveEvents OnLiveEventsChanged;
    FOnInbox OnInboxChanged;

private:
    void Open();
    void HandleMessage(const FString& Text);
    void ScheduleReconnect();
    void StopTimers();
    FString BuildUrl() const;

    TSharedRef<FZanshinClient> Client;
    TArray<FString> FlagNames;
    TSharedPtr<IWebSocket> Socket;
    TMap<FString, FZanshinFlag> Flags;
    bool bConnected = false;
    bool bClosed = false;
    int32 Attempt = 0;
    FTSTicker::FDelegateHandle PingHandle;
    FTSTicker::FDelegateHandle ReconnectHandle;
};
