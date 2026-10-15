module github.com/rep0rtDev/zanshin/nakama

go 1.22

require (
	github.com/heroiclabs/nakama-common v1.33.0
	github.com/rep0rtDev/zanshin/sdk/go v0.0.0
)

require (
	github.com/coder/websocket v1.8.12 // indirect
	github.com/klauspost/cpuid/v2 v2.0.9 // indirect
	github.com/zeebo/xxh3 v1.0.2 // indirect
	google.golang.org/protobuf v1.34.1 // indirect
)

replace github.com/rep0rtDev/zanshin/sdk/go => ../sdk/go
