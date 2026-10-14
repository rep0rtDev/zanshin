#include "ZanshinModule.h"
#include "WebSocketsModule.h"

DEFINE_LOG_CATEGORY(LogZanshin);

void FZanshinModule::StartupModule()
{
    FModuleManager::Get().LoadModuleChecked("WebSockets");
}

void FZanshinModule::ShutdownModule() {}

IMPLEMENT_MODULE(FZanshinModule, Zanshin)
