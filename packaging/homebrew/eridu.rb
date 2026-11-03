# Homebrew formula for Galaktis/homebrew-tap (`brew install galaktis/tap/eridu`).
# A template: `.github/workflows/publish.yml` fills in the version and checksums from the
# published release and pushes the result to the tap as `Formula/eridu.rb`.
class Eridu < Formula
  desc "Epic of your Repository: a 3D Git history visualizer"
  homepage "https://eridu.galaktis.eu"
  version "@VERSION@"
  license "MIT"

  on_macos do
    # One universal binary for Apple silicon and Intel.
    url "https://github.com/Galaktis/eridu/releases/download/v@VERSION@/eridu-@VERSION@-macos-universal.tar.gz"
    sha256 "@SHA256_MACOS@"
  end

  on_linux do
    on_intel do
      url "https://github.com/Galaktis/eridu/releases/download/v@VERSION@/eridu-@VERSION@-linux-x86_64.tar.gz"
      sha256 "@SHA256_LINUX@"
    end
  end

  depends_on "git"

  def install
    bin.install "eridu"
  end

  test do
    assert_match "Usage: eridu", shell_output("#{bin}/eridu --help")
  end
end
