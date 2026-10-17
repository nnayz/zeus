cask "taskmaster" do
  version "0.2.0"
  # Fail closed until the first Taskmaster release replaces this value with the
  # SHA-256 reported by GitHub. taskmaster/scripts/publish-homebrew-cask.sh owns it.
  sha256 "0000000000000000000000000000000000000000000000000000000000000000"

  url "https://github.com/nnayz/taskmaster/releases/download/v#{version}/taskmaster-#{version}-universal.dmg"
  name "Taskmaster"
  desc "Native orchestrator for coding agents"
  homepage "https://github.com/nnayz/taskmaster"

  livecheck do
    url :url
    strategy :github_latest
  end

  # Taskmaster ships its own signed updater. Homebrew installs the app initially,
  # then leaves subsequent updates to Taskmaster.
  auto_updates true
  depends_on macos: :sequoia

  app "taskmaster.app"

  # Keep ~/Library/Application Support/Taskmaster: it contains session and host state.
  # Uninstalling the client must not destroy sessions that can be reattached.
  zap trash: [
    "~/Library/Application Support/taskmaster",
    "~/Library/Caches/taskmaster",
    "~/Library/Preferences/com.taskmaster.taskmaster.plist",
    "~/Library/Saved Application State/com.taskmaster.taskmaster.savedState",
  ]
end
