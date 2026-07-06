# Homebrew formula for furl.
#
# This is a template for the o1x3/homebrew-tap repository. At release time the
# `url` and `sha256` placeholders below are filled in per platform, pointing at
# the release tarballs produced by .github/workflows/release.yml. Bump `version`
# and refresh every sha256 for each new release.
#
# The binaries are self-contained (static-ish; no runtime deps to declare), so
# this formula installs the prebuilt release archives directly rather than
# building from source.
class Furl < Formula
  desc "Human-friendly command-line HTTP client for the API era"
  homepage "https://github.com/o1x3/furl"
  version "0.1.0"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    on_arm do
      url "https://github.com/o1x3/furl/releases/download/v#{version}/furl-v#{version}-aarch64-apple-darwin.tar.gz"
      sha256 "REPLACE_WITH_AARCH64_APPLE_DARWIN_SHA256"
    end
    on_intel do
      url "https://github.com/o1x3/furl/releases/download/v#{version}/furl-v#{version}-x86_64-apple-darwin.tar.gz"
      sha256 "REPLACE_WITH_X86_64_APPLE_DARWIN_SHA256"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/o1x3/furl/releases/download/v#{version}/furl-v#{version}-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "REPLACE_WITH_AARCH64_LINUX_GNU_SHA256"
    end
    on_intel do
      url "https://github.com/o1x3/furl/releases/download/v#{version}/furl-v#{version}-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "REPLACE_WITH_X86_64_LINUX_GNU_SHA256"
    end
  end

  def install
    # Release archives extract into a "furl-v<version>-<target>" directory.
    prefix_dir = Dir["furl-v#{version}-*"].first || "."
    bin.install "#{prefix_dir}/furl"
    bin.install "#{prefix_dir}/furls"
    bin.install "#{prefix_dir}/furl-manager"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/furl --version")
  end
end
