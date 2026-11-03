# Releasing Eridu

## Every release

1. Set the version in `Cargo.toml` (and run `cargo build` so `Cargo.lock` follows), commit.
2. Tag it and push the tag:

   ```sh
   git tag v0.1.0 && git push origin v0.1.0
   ```

   `release.yml` checks that the tag matches `Cargo.toml`, builds Windows x86_64, Linux
   x86_64 and a universal macOS binary, and makes a **draft** release with the archives and
   `SHA256SUMS.txt`.
3. Try the archives, edit the generated notes, then **publish** the draft on GitHub.
4. Publishing runs `publish.yml`, which sends the release to:
   - **crates.io**: `cargo install eridu` / `cargo binstall eridu` (binstall downloads the
     release archive, see `[package.metadata.binstall]` in `Cargo.toml`);
   - **Homebrew**: updates `Formula/eridu.rb` in `Galaktis/homebrew-tap`;
   - **winget**: opens a pull request on `microsoft/winget-pkgs` (merged by their bots and
     moderators, usually within a day or two).

   A channel whose secret isn't set is skipped. Pre-releases go nowhere.

A crates.io version can never be deleted (only yanked), so check the draft before publishing.

## One-time setup

All secrets go in the repository's *Settings → Secrets and variables → Actions*.

### crates.io

1. Sign in to <https://crates.io> with GitHub. To own `eridu` as Galaktis rather than as a
   person, add a GitHub team of the Galaktis organization as an owner after the first publish
   (`cargo owner --add github:Galaktis:<team> eridu`).
2. Create an API token with the `publish-new` and `publish-update` scopes, limited to `eridu`.
3. Save it as the `CARGO_REGISTRY_TOKEN` secret.

### Homebrew tap

1. Create the public repository `Galaktis/homebrew-tap` (the `homebrew-` prefix is what makes
   `brew install galaktis/tap/eridu` work), with a README.
2. Create a fine-grained token with *Contents: read and write* on that repository only.
3. Save it as the `HOMEBREW_TAP_TOKEN` secret.

### winget

winget only automates updates to a package it already knows, so the first version is sent by
hand:

1. Fork `microsoft/winget-pkgs` into the Galaktis organization.
2. After the first release is published, copy `packaging/winget/*.yaml` to
   `manifests/g/Galaktis/Eridu/<version>/` in the fork, replace `@VERSION@` and `@SHA256@`
   (the `windows-x86_64.zip` line of `SHA256SUMS.txt`, in capitals), and check it on Windows:

   ```powershell
   winget validate --manifest manifests\g\Galaktis\Eridu\<version>
   winget install --manifest manifests\g\Galaktis\Eridu\<version>
   ```

   Then open the pull request. (`komac new Galaktis.Eridu` does the same interactively.)
3. Create a classic token with the `public_repo` scope (for the account that can push to the
   fork) and save it as the `WINGET_TOKEN` secret. From then on, `publish.yml` sends each new
   version.
