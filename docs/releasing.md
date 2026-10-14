# Releasing

Releases are cut from `main` by pushing a tag. Everything else — binaries, images, SBOMs,
signatures, the GitHub Release — is produced by `.github/workflows/release.yml`.

## Server

1. Bump `version` in the workspace `Cargo.toml` (all crates inherit it), move the `[Unreleased]`
   notes in `CHANGELOG.md` under a `## [X.Y.Z] - YYYY-MM-DD` heading, open and merge the PR.
2. Tag and push:

   ```sh
   git tag -a vX.Y.Z -m "vX.Y.Z"
   git push origin vX.Y.Z
   ```

   The workflow refuses tags whose version does not match `Cargo.toml`.
3. The `release` workflow then publishes:

   | Artifact | Where |
   | --- | --- |
   | `zanshin-X.Y.Z-{linux,darwin}-{amd64,arm64}.tar.gz`, `zanshin-X.Y.Z-windows-amd64.zip` | GitHub Release |
   | `ghcr.io/<owner>/zanshin:X.Y.Z`, `:X.Y`, `:latest` (linux/amd64 + linux/arm64) | GHCR, with SBOM + SLSA provenance attestations |
   | `ghcr.io/<owner>/zanshin-nakama:X.Y.Z-nakama3.22.0`, `:X.Y.Z`, `:latest` | GHCR — Nakama with the Zanshin runtime module |
   | `zanshin-X.Y.Z.spdx.json` | GitHub Release — SPDX SBOM of the source tree |
   | `zanshin-unreal-plugin-X.Y.Z.zip`, `zanshin-godot-addon-X.Y.Z.zip`, `zanshin-unity-upm-X.Y.Z.zip` | GitHub Release |
   | `SHA256SUMS` + `SHA256SUMS.sigstore.json` | GitHub Release — keyless cosign signature bundle |

Images are built from binaries compiled natively per architecture (`Dockerfile.release`); the
plain `Dockerfile` still builds from source for local use and CI.

### Verifying what you run

```sh
# image signature (keyless: the certificate identity is the release workflow of this repository)
cosign verify \
  --certificate-identity-regexp 'https://github.com/rep0rtDev/zanshin/' \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com \
  ghcr.io/rep0rtdev/zanshin:X.Y.Z

# provenance attestation
gh attestation verify oci://ghcr.io/rep0rtdev/zanshin:X.Y.Z --owner rep0rtDev

# release files
cosign verify-blob --bundle SHA256SUMS.sigstore.json \
  --certificate-identity-regexp 'https://github.com/rep0rtDev/zanshin/' \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com SHA256SUMS
sha256sum -c SHA256SUMS --ignore-missing

# what a running server was built from
zanshin --version
curl -s -H "x-api-key: $ADMIN_KEY" http://localhost:7550/admin/v1/system | jq .build
```

## SDKs

| SDK | How it ships |
| --- | --- |
| TypeScript (`@zanshin/sdk`) | bump `sdk/ts/package.json`, tag `sdk-ts-vX.Y.Z` → `publish-sdk-ts.yml` publishes to npm with provenance (needs the `NPM_TOKEN` secret) |
| Go (`github.com/rep0rtDev/zanshin/sdk/go`) | tag `sdk/go/vX.Y.Z`; the Go module proxy and pkg.go.dev pick it up |
| Unity | UPM git dependency: `https://github.com/rep0rtDev/zanshin.git?path=sdk/unity#vX.Y.Z`; the release also carries a zip |
| Unreal | `zanshin-unreal-plugin-X.Y.Z.zip` from the release → `Plugins/Zanshin` |
| Godot | `zanshin-godot-addon-X.Y.Z.zip` from the release → `addons/zanshin` |
| Nakama module | `ghcr.io/<owner>/zanshin-nakama` image, or build `nakama/Dockerfile` with your Nakama version |

## Supply chain checks

`security.yml` runs on every PR, on `main` and weekly:

- `cargo deny check` (`deny.toml`): RustSec advisories, license allow-list, yanked crates,
  registry sources;
- `cargo audit` (RustSec);
- `npm audit --audit-level=high` for the console and the TypeScript SDK;
- `govulncheck` for the Go SDK and the Nakama module;
- `trivy fs` (vulnerabilities, secrets, IaC misconfiguration) on the repository.

Dependency versions are pinned through `Cargo.lock`, `package-lock.json` and `go.sum`; release
builds use `--locked`.

## Versioning

SemVer on the server. The REST/gRPC API is stable within a major version; config keys are only
added, never renamed, within a minor. SDKs version independently.
