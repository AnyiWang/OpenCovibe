#!/usr/bin/env bash
#
# OpenCovibe Desktop — Development Environment Setup
#
# Detects missing dependencies, installs them, and prepares the project.
# macOS only. Run: chmod +x scripts/setup.sh && ./scripts/setup.sh
#
# Options:
#   --yes    Skip all confirmation prompts (auto-accept)

# ---------------------------------------------------------------------------
# Globals
# ---------------------------------------------------------------------------

AUTO_YES=false
for arg in "$@"; do
  case "$arg" in
    --yes|-y) AUTO_YES=true ;;
  esac
done

# Colors — 256-color with TTY and NO_COLOR protection
if [[ -t 1 ]] && [ -z "${NO_COLOR:-}" ]; then
  BRAND='\033[38;5;214m'      # Brand color (golden amber, matches app primary)
  GREEN='\033[38;5;71m'       # Success (soft green)
  RED='\033[38;5;167m'        # Failure (soft red)
  DIM='\033[38;5;245m'        # Secondary info (gray)
  BOLD='\033[1m'
  NC='\033[0m'
else
  BRAND='' GREEN='' RED='' DIM='' BOLD='' NC=''
fi

# Track what was freshly installed (for PATH refresh hints at the end)
INSTALLED_BREW=false
INSTALLED_RUST=false

# Resolve project root reliably (handles symlinks and sourced execution)
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

ok()     { printf "${GREEN}✓${NC} %s\n" "$1"; }
info()   { printf "${BRAND}→${NC} %s\n" "$1"; }
fail()   { printf "${RED}✗${NC} %s\n" "$1"; }
dim()    { printf "${DIM}%s${NC}\n" "$1"; }
header() { printf "\n${BRAND}${BOLD}═══ %s ═══${NC}\n\n" "$1"; }

# Ask y/n. Returns 0 for yes, 1 for no. --yes mode always returns 0.
confirm() {
  if $AUTO_YES; then
    return 0
  fi
  printf "%s [y/N] " "$1"
  read -r answer
  case "$answer" in
    [yY]|[yY][eE][sS]) return 0 ;;
    *) return 1 ;;
  esac
}

# Compare versions: version_gte "22.1.0" "20" → true (22 >= 20)
# Compares segment by segment. Missing segments treated as 0.
version_gte() {
  local IFS='.'
  local -a ver_a=($1) ver_b=($2)
  local max=${#ver_a[@]}
  if [ ${#ver_b[@]} -gt "$max" ]; then
    max=${#ver_b[@]}
  fi
  for ((i = 0; i < max; i++)); do
    local a=${ver_a[$i]:-0}
    local b=${ver_b[$i]:-0}
    if [ "$a" -gt "$b" ]; then
      return 0
    elif [ "$a" -lt "$b" ]; then
      return 1
    fi
  done
  return 0  # equal
}

# brew install with auto-retry on link conflicts.
# Handles stale symlinks from old macOS installs or manual formula remnants.
brew_install() {
  local pkg="$1"
  # Skip if already installed
  if brew list "$pkg" &>/dev/null; then
    return 0
  fi
  local output
  output=$(brew install "$pkg" 2>&1)
  local rc=$?
  echo "$output"
  if [ $rc -eq 0 ] && ! echo "$output" | grep -q "Could not symlink"; then
    return 0
  fi
  # Link conflict detected — fix and retry
  info "Fixing symlink conflicts..."
  brew link --overwrite "$pkg" 2>/dev/null
  brew install "$pkg" 2>&1
}

# After brew install node, ensure brew's node is first in PATH.
# Fixes the case where an old .pkg or manual Node install shadows brew's version.
ensure_brew_node_in_path() {
  local brew_prefix
  brew_prefix="$(brew --prefix 2>/dev/null)"
  if [ -n "$brew_prefix" ] && [ -x "$brew_prefix/bin/node" ]; then
    export PATH="$brew_prefix/bin:$PATH"
    hash -r 2>/dev/null  # clear bash's command cache
  fi
}

# Detect which Node version manager is active (nvm, fnm, volta, asdf, mise).
# Returns the manager name, or empty string if none.
detect_node_manager() {
  if [ -n "${NVM_DIR:-}" ] && [ -s "$NVM_DIR/nvm.sh" ]; then
    echo "nvm"
  elif command -v fnm &>/dev/null; then
    echo "fnm"
  elif command -v volta &>/dev/null; then
    echo "volta"
  elif command -v asdf &>/dev/null && asdf plugin list 2>/dev/null | grep -q nodejs; then
    echo "asdf"
  elif command -v mise &>/dev/null && mise plugins list 2>/dev/null | grep -q node; then
    echo "mise"
  else
    echo ""
  fi
}

# Install Node via the detected version manager.
# Returns 0 on success, 1 on failure.
install_node_via_manager() {
  local mgr="$1"
  local ver="$2"
  case "$mgr" in
    nvm)
      source "$NVM_DIR/nvm.sh"
      nvm install "$ver" && nvm use "$ver" && nvm alias default "$ver"
      ;;
    fnm)
      fnm install "$ver" && fnm use "$ver"
      ;;
    volta)
      volta install "node@$ver"
      ;;
    asdf)
      asdf install nodejs "$ver" && asdf global nodejs "$ver"
      ;;
    mise)
      mise install "node@$ver" && mise use --global "node@$ver"
      ;;
    *)
      return 1
      ;;
  esac
}

# Require a dependency or exit.
require_or_exit() {
  local name="$1"
  fail "$name is required but was not installed. Cannot continue."
  exit 1
}

# ---------------------------------------------------------------------------
# Step 0: Platform & environment checks
# ---------------------------------------------------------------------------

printf "\n${BRAND}${BOLD}  OpenCovibe Desktop — Development Setup${NC}\n\n"

if [ "$(uname -s)" != "Darwin" ]; then
  fail "This script only supports macOS. Detected: $(uname -s)"
  dim "  Linux and Windows support may be added in the future."
  exit 1
fi

ok "macOS detected $(DIM=$(sw_vers -productVersion))"

# Pull latest code if inside a git repo
if git -C "$PROJECT_DIR" rev-parse --is-inside-work-tree &>/dev/null; then
  # Reset generated lock files first (different toolchain versions cause conflicts)
  for lockfile in package-lock.json src-tauri/Cargo.lock; do
    if git -C "$PROJECT_DIR" diff --name-only 2>/dev/null | grep -q "$lockfile"; then
      info "Resetting ${lockfile} (local changes from different toolchain version)"
      git -C "$PROJECT_DIR" checkout -- "$lockfile" 2>/dev/null
    fi
  done
  # Pull latest
  info "Pulling latest code..."
  if git -C "$PROJECT_DIR" pull --ff-only 2>/dev/null; then
    ok "Code up to date"
  else
    # ff-only failed — could be diverged or unrelated local changes
    dim "  git pull --ff-only failed (local commits or diverged branch). Skipping."
  fi
fi

# Check available disk space (need ~10GB for all tools + first Rust build)
available_gb=$(df -g "$HOME" 2>/dev/null | awk 'NR==2 {print $4}')
if [ -n "$available_gb" ] && [ "$available_gb" -lt 10 ] 2>/dev/null; then
  fail "Low disk space: ${available_gb}GB available, ~10GB recommended."
  dim "  Xcode CLI Tools (~1.2GB) + Rust toolchain (~1.5GB) + node_modules + first build cache."
  if ! confirm "Continue anyway?"; then
    exit 1
  fi
fi

# ---------------------------------------------------------------------------
# Step 1: Xcode Command Line Tools
# ---------------------------------------------------------------------------

header "Step 1: Xcode CLI Tools"

xcode_ok=false
if xcode-select -p &>/dev/null; then
  # Validate that the tools actually work (path can exist but tools be broken after OS upgrade)
  if clang --version &>/dev/null && git --version &>/dev/null; then
    xcode_ok=true
    ok "Xcode CLI Tools already installed"
  else
    info "Xcode CLI Tools path exists but tools are broken (possible OS upgrade issue)"
  fi
fi

if ! $xcode_ok; then
  if [ "$xcode_ok" = false ] && ! xcode-select -p &>/dev/null; then
    info "Xcode CLI Tools not found"
  fi

  # Check if we can show a GUI dialog
  has_gui=false
  if [ -n "$DISPLAY" ] || command -v open &>/dev/null; then
    has_gui=true
  fi

  if ! $has_gui; then
    fail "Xcode CLI Tools are required but no GUI is available (SSH session?)."
    dim "  Please install on the machine directly:"
    dim "    xcode-select --install"
    dim "  Then re-run this script."
    exit 1
  fi

  if confirm "Install Xcode CLI Tools?"; then
    # Reset if path exists but tools are broken
    if xcode-select -p &>/dev/null; then
      sudo xcode-select --reset 2>/dev/null
    fi
    xcode-select --install 2>/dev/null
    info "Waiting for Xcode CLI Tools installation (this opens a system dialog)..."

    # Poll until installed or timeout (300s)
    elapsed=0
    while ! (xcode-select -p &>/dev/null && clang --version &>/dev/null); do
      if [ $elapsed -ge 300 ]; then
        fail "Timed out waiting for Xcode CLI Tools. Please install manually and re-run."
        exit 1
      fi
      sleep 5
      elapsed=$((elapsed + 5))
    done

    ok "Xcode CLI Tools installed"
  else
    require_or_exit "Xcode CLI Tools"
  fi
fi

# ---------------------------------------------------------------------------
# Step 2: Homebrew
# ---------------------------------------------------------------------------

header "Step 2: Homebrew"

if command -v brew &>/dev/null; then
  ok "Homebrew already installed"
  dim "  $(brew --version | head -1)"
else
  info "Homebrew not found"

  if confirm "Install Homebrew?"; then
    info "Installing Homebrew (sudo password may be required)..."
    /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"

    if [ $? -ne 0 ]; then
      fail "Homebrew installation failed"
      exit 1
    fi

    # Add brew to PATH for this session
    if [ -x /opt/homebrew/bin/brew ]; then
      eval "$(/opt/homebrew/bin/brew shellenv)"
    elif [ -x /usr/local/bin/brew ]; then
      eval "$(/usr/local/bin/brew shellenv)"
    fi

    if command -v brew &>/dev/null; then
      ok "Homebrew installed"
      dim "  $(brew --version | head -1)"
      INSTALLED_BREW=true
    else
      fail "Homebrew installed but not found in PATH. Please restart your terminal and re-run."
      exit 1
    fi
  else
    require_or_exit "Homebrew"
  fi
fi

# Fix stale headers in /usr/local/include that break Rust/ObjC builds.
# Old xz installs can leave a Block.h with lzma content that shadows the
# system Block.h (Objective-C blocks), causing mac-notification-sys to fail.
if [ -f /usr/local/include/Block.h ] && head -20 /usr/local/include/Block.h 2>/dev/null | grep -q "lzma"; then
  info "Removing stale /usr/local/include/Block.h (lzma remnant conflicting with system header)..."
  rm -f /usr/local/include/Block.h 2>/dev/null || sudo rm -f /usr/local/include/Block.h
  if [ $? -eq 0 ]; then
    ok "Stale Block.h removed"
  else
    fail "Could not remove /usr/local/include/Block.h — run: sudo rm /usr/local/include/Block.h"
  fi
fi

# ---------------------------------------------------------------------------
# Step 3: Node.js >= 20
# ---------------------------------------------------------------------------

header "Step 3: Node.js"

NODE_MIN="20"

if command -v node &>/dev/null; then
  node_ver=$(node --version | sed 's/^v//')
  if version_gte "$node_ver" "$NODE_MIN"; then
    ok "Node.js already installed"
    dim "  v${node_ver}"
    # Ensure nvm default alias is set to a good version (prevents reverting in new terminals)
    if [ -n "${NVM_DIR:-}" ] && [ -s "$NVM_DIR/nvm.sh" ]; then
      source "$NVM_DIR/nvm.sh" 2>/dev/null
      nvm_default=$(nvm alias default 2>/dev/null | sed 's/\x1b\[[0-9;]*m//g' | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -1)
      if [ -n "$nvm_default" ] && ! version_gte "$nvm_default" "$NODE_MIN"; then
        info "nvm default is v${nvm_default}, updating to v${node_ver}..."
        nvm alias default "$node_ver" >/dev/null
        nvm use "$node_ver" >/dev/null
        ok "nvm default set to v${node_ver}"
      fi
    fi
  else
    info "Node.js v${node_ver} found but v${NODE_MIN}+ is required"

    # Detect active Node version manager
    node_mgr=$(detect_node_manager)
    if [ -n "$node_mgr" ]; then
      if confirm "Upgrade Node.js via ${node_mgr}?"; then
        info "Installing Node.js ${NODE_MIN} via ${node_mgr}..."
        install_node_via_manager "$node_mgr" "$NODE_MIN"
        if [ $? -ne 0 ]; then
          fail "Node.js installation via ${node_mgr} failed"
          exit 1
        fi
        node_ver=$(node --version | sed 's/^v//')
        if version_gte "$node_ver" "$NODE_MIN"; then
          ok "Node.js installed via ${node_mgr} (v${node_ver})"
        else
          fail "Node.js is still v${node_ver} after ${node_mgr} install."
          exit 1
        fi
      else
        require_or_exit "Node.js >= ${NODE_MIN}"
      fi
    elif confirm "Upgrade Node.js via Homebrew?"; then
      info "Installing Node.js..."
      brew_install node
      if [ $? -ne 0 ]; then
        fail "Node.js installation failed"
        exit 1
      fi
      # Force brew's node to overwrite old pkg-installed node binary
      brew link --overwrite node 2>/dev/null
      ensure_brew_node_in_path
      node_ver=$(node --version | sed 's/^v//')
      if ! version_gte "$node_ver" "$NODE_MIN"; then
        fail "Node.js is still v${node_ver} after install. An old Node installation may be shadowing Homebrew's version."
        dim "  Try: nvm install ${NODE_MIN} (if using nvm), or remove the old Node.js and re-run."
        exit 1
      fi
      ok "Node.js installed (v${node_ver})"
    else
      require_or_exit "Node.js >= ${NODE_MIN}"
    fi
  fi
else
  info "Node.js not found"

  if confirm "Install Node.js via Homebrew?"; then
    info "Installing Node.js..."
    brew_install node
    if [ $? -ne 0 ]; then
      fail "Node.js installation failed"
      exit 1
    fi
    brew link --overwrite node 2>/dev/null
    ensure_brew_node_in_path
    node_ver=$(node --version | sed 's/^v//')
    ok "Node.js installed (v${node_ver})"
  else
    require_or_exit "Node.js"
  fi
fi

# ---------------------------------------------------------------------------
# Step 4: Rust (cargo via rustup)
# ---------------------------------------------------------------------------

header "Step 4: Rust"

if command -v cargo &>/dev/null; then
  # Verify a toolchain is actually installed (rustup can exist without a toolchain)
  if rustc --version &>/dev/null; then
    ok "Rust already installed"
    dim "  $(rustc --version)"
  else
    info "rustup found but no toolchain installed"
    info "Installing stable toolchain..."
    rustup toolchain install stable
    rustup default stable
    if rustc --version &>/dev/null; then
      ok "Rust toolchain installed ($(rustc --version))"
    else
      fail "Failed to install Rust toolchain"
      exit 1
    fi
  fi
elif command -v rustup &>/dev/null; then
  # rustup exists but cargo is not on PATH (custom CARGO_HOME?)
  info "rustup found but cargo not in PATH"
  if [ -f "${CARGO_HOME:-$HOME/.cargo}/env" ]; then
    source "${CARGO_HOME:-$HOME/.cargo}/env"
  fi
  if command -v cargo &>/dev/null; then
    ok "Rust already installed"
    dim "  $(rustc --version 2>/dev/null || echo 'unknown')"
  else
    fail "Could not locate cargo. Check your CARGO_HOME setting."
    exit 1
  fi
else
  info "Rust (cargo) not found"

  if confirm "Install Rust via rustup?"; then
    info "Installing Rust..."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
    if [ $? -ne 0 ]; then
      fail "Rust installation failed"
      exit 1
    fi

    # Add cargo to PATH for this session (respect custom CARGO_HOME)
    if [ -f "${CARGO_HOME:-$HOME/.cargo}/env" ]; then
      source "${CARGO_HOME:-$HOME/.cargo}/env"
    fi

    if command -v cargo &>/dev/null && rustc --version &>/dev/null; then
      ok "Rust installed ($(rustc --version))"
      INSTALLED_RUST=true
    else
      fail "Rust installed but cargo not found in PATH. Please restart your terminal and re-run."
      exit 1
    fi
  else
    require_or_exit "Rust"
  fi
fi

# ---------------------------------------------------------------------------
# Step 5: Claude Code CLI (optional — not required for build)
# ---------------------------------------------------------------------------

header "Step 5: Claude Code CLI"

claude_found=false
claude_ver=""

# Check PATH first, then ~/.local/bin (native installer location)
if command -v claude &>/dev/null; then
  claude_ver=$(claude --version 2>/dev/null | head -1)
  claude_found=true
elif [ -x "$HOME/.local/bin/claude" ]; then
  claude_ver=$("$HOME/.local/bin/claude" --version 2>/dev/null | head -1)
  claude_found=true
fi

if $claude_found; then
  ok "Claude Code CLI already installed"
  dim "  ${claude_ver}"
else
  info "Claude Code CLI not found"
  dim "  Claude Code CLI is optional for development but required at runtime."

  # Build list of available install methods
  methods=()
  method_cmds=()
  method_labels=()

  if command -v brew &>/dev/null; then
    methods+=("brew")
    method_cmds+=("brew install claude-code")
    method_labels+=("brew install claude-code")
  fi

  if command -v npm &>/dev/null; then
    node_major=$(node --version 2>/dev/null | sed 's/^v//' | cut -d. -f1)
    if [ -n "$node_major" ] && [ "$node_major" -ge 18 ] 2>/dev/null; then
      methods+=("npm")
      method_cmds+=("npm install -g @anthropic-ai/claude-code")
      method_labels+=("npm install -g @anthropic-ai/claude-code")
    fi
  fi

  if command -v curl &>/dev/null; then
    methods+=("curl")
    method_cmds+=("curl -fsSL https://claude.ai/install.sh | bash")
    method_labels+=("curl -fsSL https://claude.ai/install.sh | bash")
  fi

  if [ ${#methods[@]} -eq 0 ]; then
    fail "No install method available (need brew, npm, or curl)"
    dim "  Install one of these tools first, or install Claude Code manually."
  else
    if $AUTO_YES; then
      # Auto mode: use first available method
      choice_idx=0
      info "Auto-installing via: ${method_labels[$choice_idx]}"
    else
      echo ""
      echo "  Available install methods:"
      for i in "${!methods[@]}"; do
        n=$((i + 1))
        if [ $i -eq 0 ]; then
          printf "    ${BRAND}[%d]${NC} %s  ${DIM}(recommended)${NC}\n" "$n" "${method_labels[$i]}"
        else
          printf "    ${BRAND}[%d]${NC} %s\n" "$n" "${method_labels[$i]}"
        fi
      done
      printf "    ${DIM}[s] Skip (install later)${NC}\n"
      echo ""
      printf "  Choose [1-%d/s]: " "${#methods[@]}"
      read -r choice

      case "$choice" in
        [sS]) choice_idx=-1 ;;
        *)
          if [[ "$choice" =~ ^[0-9]+$ ]] && [ "$choice" -ge 1 ] && [ "$choice" -le "${#methods[@]}" ]; then
            choice_idx=$((choice - 1))
          else
            choice_idx=-1
            dim "  Invalid choice, skipping."
          fi
          ;;
      esac
    fi

    if [ "${choice_idx:-}" != "-1" ] && [ -n "${choice_idx:-}" ]; then
      selected_method="${methods[$choice_idx]}"
      selected_cmd="${method_cmds[$choice_idx]}"
      info "Installing via: ${selected_cmd}"

      # Set env vars for cleaner output
      export HOMEBREW_NO_AUTO_UPDATE=1

      if eval "$selected_cmd"; then
        # Verify installation
        hash -r 2>/dev/null
        if command -v claude &>/dev/null; then
          claude_ver=$(claude --version 2>/dev/null | head -1)
          ok "Claude Code CLI installed"
          dim "  ${claude_ver}"
        elif [ -x "$HOME/.local/bin/claude" ]; then
          claude_ver=$("$HOME/.local/bin/claude" --version 2>/dev/null | head -1)
          ok "Claude Code CLI installed"
          dim "  ${claude_ver}"
          dim "  Note: Add ~/.local/bin to your PATH for easy access."
        else
          fail "Install command succeeded but 'claude' not found in PATH"
          dim "  You may need to restart your terminal or add the install location to PATH."
        fi
      else
        fail "Installation failed"
        dim "  You can install Claude Code CLI later. The app will guide you."
      fi
    else
      dim "  Skipped. You can install Claude Code CLI later via the app."
    fi
  fi
fi

# ---------------------------------------------------------------------------
# Step 6: npm install
# ---------------------------------------------------------------------------

header "Step 6: Project Dependencies"

if [ ! -f "$PROJECT_DIR/package.json" ]; then
  fail "Cannot find package.json in ${PROJECT_DIR}"
  exit 1
fi

cd "$PROJECT_DIR" || exit 1

info "Running npm install..."
npm install
if [ $? -ne 0 ]; then
  # If using a non-official registry, offer to switch and retry
  npm_registry=$(npm config get registry 2>/dev/null)
  if [ -n "$npm_registry" ] && ! echo "$npm_registry" | grep -q "registry.npmjs.org"; then
    info "npm install failed. Current registry: ${npm_registry}"
    if confirm "Switch to official npm registry (https://registry.npmjs.org/) and retry?"; then
      npm config set registry https://registry.npmjs.org/
      ok "Switched to official npm registry"
      info "Retrying npm install..."
      npm install
      if [ $? -ne 0 ]; then
        fail "npm install failed"
        exit 1
      fi
    else
      fail "npm install failed"
      exit 1
    fi
  else
    fail "npm install failed"
    exit 1
  fi
fi
ok "npm dependencies installed"

# ---------------------------------------------------------------------------
# Step 7: Smoke test
# ---------------------------------------------------------------------------

info "Verifying Tauri CLI..."
if npx tauri --version &>/dev/null; then
  ok "Tauri CLI works"
  dim "  $(npx tauri --version 2>/dev/null)"
else
  fail "Tauri CLI smoke test failed. Try: npm rebuild"
fi

# ---------------------------------------------------------------------------
# Done
# ---------------------------------------------------------------------------

echo ""
printf "${GREEN}${BOLD}  Setup complete!${NC}\n"
echo ""

# Build a source command to refresh PATH in the current terminal
source_cmds=""
if $INSTALLED_BREW; then
  if [ -x /opt/homebrew/bin/brew ]; then
    source_cmds='eval "$(/opt/homebrew/bin/brew shellenv)"'
  elif [ -x /usr/local/bin/brew ]; then
    source_cmds='eval "$(/usr/local/bin/brew shellenv)"'
  fi
fi
if $INSTALLED_RUST; then
  if [ -n "$source_cmds" ]; then
    source_cmds="$source_cmds && source ~/.cargo/env"
  else
    source_cmds="source ~/.cargo/env"
  fi
fi

if confirm "Start the development environment now? (first Rust build may take a few minutes)"; then
  exec npm run tauri dev
else
  echo ""
  dim "  To start later:"
  if [ -n "$source_cmds" ]; then
    echo ""
    dim "    # Option 1: open a new terminal tab, then:"
    dim "    npm run tauri dev"
    echo ""
    dim "    # Option 2: stay in this terminal:"
    dim "    $source_cmds && npm run tauri dev"
  else
    echo ""
    dim "    npm run tauri dev"
  fi
  echo ""
fi
