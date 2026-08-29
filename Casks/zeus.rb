cask "zeus" do
  version "0.2.0"
  # Fail closed until the first Zeus release replaces this value with the
  # SHA-256 reported by GitHub. zeus/scripts/publish-homebrew-cask.sh owns it.
  sha256 "0000000000000000000000000000000000000000000000000000000000000000"

  url "https://github.com/nnayz/zeus/releases/download/v#{version}/zeus-#{version}-universal.dmg"
  name "Zeus"
  desc "Native orchestrator for coding agents"
  homepage "https://github.com/nnayz/zeus"

  livecheck do
    url :url
    strategy :github_latest
  end

  # Zeus ships its own signed updater. Homebrew installs the app initially,
  # then leaves subsequent updates to Zeus.
  auto_updates true
  depends_on macos: :sequoia

  app "zeus.app"

  # Keep ~/Library/Application Support/Zeus: it contains session and host state.
  # Uninstalling the client must not destroy sessions that can be reattached.
  zap trash: [
    "~/Library/Application Support/zeus",
    "~/Library/Caches/zeus",
    "~/Library/Preferences/com.zeus.zeus.plist",
    "~/Library/Saved Application State/com.zeus.zeus.savedState",
  ]
end
