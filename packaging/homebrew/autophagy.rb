# Homebrew formula TEMPLATE for the `autophagy` CLI.
#
# There is no Homebrew tap for this project yet. This file is not installed by
# `brew` from anywhere today; it is a starting point for the user to drop into
# a future `karnstack/homebrew-autophagy` tap (or `brew create`-style local
# install) once a `v0.1.0` tag exists.
#
# Scope: this formula builds and installs the `autophagy` command-line tool
# only, from source, via the pinned Rust toolchain. It intentionally does NOT
# build the native macOS app in `apps/macos/` — that is a separate SwiftUI
# app bundle (not a Homebrew-shaped console binary), best distributed as the
# `autophagy-macos-arm64-app.tar.gz` artifact already produced by
# `.github/workflows/release.yml`, or eventually its own `--cask`. Keeping this
# formula to the CLI keeps it simple, cross-platform (macOS + Linux), and
# buildable with just Rust — no Xcode dependency for `brew install`.
#
# Before use at release time:
#   1. Push the `v0.1.0` tag and let .github/workflows/release.yml publish
#      source/binary artifacts (or rely on the GitHub-generated source tarball
#      for the tag).
#   2. Replace PLACEHOLDER_URL below with the real tarball URL for the tag,
#      e.g. https://github.com/karnstack/autophagy/archive/refs/tags/v0.1.0.tar.gz
#   3. Compute the real sha256 (`shasum -a 256 <tarball>`) and replace
#      PLACEHOLDER_SHA256.
#   4. Move this file into a tap repository as
#      `Formula/autophagy.rb` (e.g. karnstack/homebrew-autophagy), then
#      `brew install karnstack/autophagy/autophagy`.
class Autophagy < Formula
  desc "Local-first behavioral improvement layer for coding agents (CLI)"
  homepage "https://github.com/karnstack/autophagy"
  url "PLACEHOLDER_URL" # e.g. https://github.com/karnstack/autophagy/archive/refs/tags/v0.1.0.tar.gz
  sha256 "PLACEHOLDER_SHA256" # shasum -a 256 of the tarball above
  license "Apache-2.0"
  head "https://github.com/karnstack/autophagy.git", branch: "main"

  depends_on "rust" => :build

  def install
    # Builds only the `autophagy-cli` binary crate; the macOS app is a
    # separate Swift package and is not built by this formula.
    system "cargo", "install", *std_cargo_args(path: "crates/autophagy-cli")
  end

  test do
    # `autophagy --version` should never touch the network or a database.
    assert_match version.to_s, shell_output("#{bin}/autophagy --version")
  end
end
