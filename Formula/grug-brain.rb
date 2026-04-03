class GrugBrain < Formula
  desc "Persistent memory for LLMs — FTS5 search, git sync, markdown storage"
  homepage "https://github.com/ryanthedev/grug-brain.mcp"
  url "https://github.com/ryanthedev/grug-brain.mcp/archive/refs/tags/v4.0.0.tar.gz"
  sha256 "PLACEHOLDER_SHA256"
  license "MIT"

  depends_on "rust" => :build

  def install
    system "cargo", "install", *std_cargo_args
  end

  def caveats
    <<~EOS
      To start grug-brain as a background service:
        grug serve --install-service

      To register with Claude Code:
        claude plugin add rtd/grug-brain
        /setup

      Configuration: ~/.grug-brain/brains.json
    EOS
  end

  test do
    assert_match "grug-brain", shell_output("#{bin}/grug --help")
  end
end
