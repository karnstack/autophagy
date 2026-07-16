# Homebrew packaging (template, no tap yet)

`autophagy.rb` in this directory is a **template** Homebrew formula for the
`autophagy` command-line tool. It is not published anywhere and `brew` cannot
install it from this location — there is no `karnstack/homebrew-autophagy`
tap today. It exists so the exact release steps are captured for whoever cuts
the first real release.

It builds only the CLI, from source, with the pinned Rust toolchain (via
`depends_on "rust" => :build"` and `cargo install`). The native macOS app in
`apps/macos/` is a separate SwiftUI package and is intentionally out of scope
for this formula; it ships as its own `.app` bundle artifact from
`.github/workflows/release.yml` (or could become a `--cask` later).

## Using it after tagging `v0.1.0`

1. Push the `v0.1.0` tag (this repository's release workflow only drafts a
   GitHub release on a tag push — a human still publishes it manually).
2. Edit `autophagy.rb`:
   - Replace `PLACEHOLDER_URL` with the tag's source tarball URL, e.g.
     `https://github.com/karnstack/autophagy/archive/refs/tags/v0.1.0.tar.gz`.
   - Compute the real checksum and replace `PLACEHOLDER_SHA256`:
     ```sh
     curl -L -o autophagy-0.1.0.tar.gz \
       https://github.com/karnstack/autophagy/archive/refs/tags/v0.1.0.tar.gz
     shasum -a 256 autophagy-0.1.0.tar.gz
     ```
3. Create a tap repository (e.g. `karnstack/homebrew-autophagy`) with a
   `Formula/` directory, and copy the filled-in `autophagy.rb` there as
   `Formula/autophagy.rb`.
4. Users install with:
   ```sh
   brew tap karnstack/autophagy
   brew install autophagy
   ```
5. Validate locally before pushing the tap:
   ```sh
   brew install --build-from-source ./autophagy.rb
   brew test autophagy
   brew audit --strict --online autophagy
   ```

If a `brew audit` run flags anything (formula naming, `url`/`sha256` format,
missing `test do` coverage), fix it in the tap, not here — this file is a
starting point, not the final formula.
