# 🎵 Raspberry Discord Bot With Rust

High-performance Discord bot for Raspberry Pi written in Rust. Features music playback from YouTube, automatic reconnection, and systemd integration for production deployment.

**Language:** Rust | **Runtime:** Raspberry Pi / Linux | **Status:** Production-ready

---

## 🎯 Features

| Feature | Description |
|---------|-------------|
| `!ping` | Quick health check - responds with `Pong!` |
| `!play <url>` | Join voice channel and stream audio from YouTube URL |
| `!yt <url>` | Alias for `!play` |
| `!stop` | Stop current playback |
| `!leave` | Disconnect from voice channel |
| `!ytdownload <url>` | Download and attach video (within Discord upload limits) |
| **Auto-Reconnect** | Automatically reconnects if Discord session drops |
| **systemd Ready** | Pre-configured service file for Raspberry Pi |
| **Optimized Binary** | Release build with LTO, strip, and optimized codegen |
| **Error Resilience** | Graceful handling of invalid tokens (401 detection) and missing dependencies |

## 🛠️ Technology Stack

- **Language:** Rust 2024 Edition
- **Async Runtime:** Tokio
- **Discord Client:** Serenity 0.12.5+
- **Audio Framework:** Songbird (Serenity's voice extension)
- **Media Download:** yt-dlp
- **Audio Processing:** ffmpeg

## 📋 System Requirements

### Hardware
- Raspberry Pi 3B+ or better (4B+ recommended for faster builds)
- 1+ GB RAM available
- Stable internet connection

### Software Dependencies (Linux/RPi)

```bash
sudo apt update && sudo apt install -y \
  build-essential pkg-config libssl-dev \
  ffmpeg python3 python3-pip
  
# Install yt-dlp
python3 -m pip install -U yt-dlp

# Install Rust (run as regular user, not root)
curl https://sh.rustup.rs -sSf | sh
source "$HOME/.cargo/env"
```

**macOS:**
```bash
brew install openssl ffmpeg python3
python3 -m pip install -U yt-dlp
curl https://sh.rustup.rs -sSf | sh
```

**Windows:**
Use WSL2 or Windows Terminal with the Linux instructions above.

## ⚙️ Configuration

### 1. Discord Developer Portal Setup

1. Go to [Discord Developer Portal](https://discord.com/developers/applications)
2. Click **New Application** and fill in a name (e.g., "RaspberryBot")
3. Go to **Bot** section → Click **Add Bot**
4. Under **TOKEN**, copy your bot token
5. Enable these Gateway Intents:
   - ✅ **MESSAGE CONTENT INTENT** (required)
   - Optional: Server Members Intent, Presence Intent
6. Go to **OAuth2 → URL Generator**:
   - Scopes: `bot`
   - Permissions: `Send Messages`, `Connect`, `Speak`, `Attach Files`, `View Channels`
7. Copy the generated OAuth2 URL and open it to invite your bot to your server

### 2. Environment Setup

Copy `.env.example` to `.env` and set your real token:

```bash
cp .env.example .env
```

Then edit `.env`:

```env
DISCORD_TOKEN=your_bot_token_here
RUST_LOG=info
```

⚠️ **SECURITY WARNING:** Never commit `.env` to version control. It's already in `.gitignore`.

## 🚀 Quick Start

### Clone and Build

```bash
git clone https://github.com/devcepeda/raspberry-discord-with-rust.git
cd raspberry-discord-with-rust

# Debug build (faster compilation, slower execution)
cargo build

# Release build (slower compilation, optimized runtime)
cargo build --release
```

### Run the Bot

```bash
# Using cargo
cargo run --release

# Or directly from the compiled binary
./target/release/discord-bot
```

The bot will connect to Discord using the token from `.env` and display:
```
[INFO] Bot logged in as: YourBotName#1234
[INFO] Ready!
```

## 📝 Development Workflow

### Code Quality

Always run these before committing:

```bash
# Check formatting
cargo fmt --check

# Lint and optimization suggestions
cargo clippy --all-targets --all-features -- -D warnings

# Run tests (if any)
cargo test
```

### Full Development Cycle

```bash
# Install git hooks (optional)
cargo install cargo-husky

# Development iteration
cargo fmt                    # Format code
cargo clippy                 # Check for issues
cargo build                  # Compile debug
RUST_LOG=debug cargo run     # Run with debug logging

# Before production
cargo build --release        # Compile optimized
cargo test --release        # Validate
```

## 🔧 Production Deployment (Raspberry Pi)

### Install as a systemd Service

The project includes a pre-configured systemd service file. Installation:

```bash
# 1. Build the optimized release binary
cargo build --release

# 2. Copy service file
sudo cp deploy/discord-bot.service /etc/systemd/system/discord-bot.service

# 3. Copy .env to home directory (systemd runs as pi user)
cp .env ~/discord-bot-env
sudo chown pi:pi ~/discord-bot-env && sudo chmod 600 ~/discord-bot-env

# 4. Update service file paths if needed
sudo nano /etc/systemd/system/discord-bot.service
# Ensure ExecStart points to correct binary path and .env location

# 5. Enable and start the service
sudo systemctl daemon-reload
sudo systemctl enable discord-bot
sudo systemctl start discord-bot
```

### Monitor the Service

```bash
# Check status
sudo systemctl status discord-bot

# View live logs
journalctl -u discord-bot -f

# View last 50 lines
journalctl -u discord-bot -n 50

# Stop the service
sudo systemctl stop discord-bot

# Restart the service
sudo systemctl restart discord-bot
```

### Service File Details

The service is configured to:
- Run as user `pi` ✓
- Auto-start on boot ✓
- Auto-restart on crash ✓
- Log to systemd journal ✓
- Use `.env` for Discord token ✓

## 🎮 Available Commands

| Command | Args | Example | Description |
|---------|------|---------|-------------|
| `!ping` | None | `!ping` | Replies with `Pong!` to verify bot is responsive |
| `!play` | YouTube URL | `!play https://www.youtube.com/watch?v=...` | Join your voice channel and stream audio |
| `!yt` | YouTube URL | `!yt https://www.youtube.com/watch?v=...` | Alias for `!play` |
| `!stop` | None | `!stop` | Stop current playback |
| `!leave` | None | `!leave` | Disconnect from voice channel |
| `!ytdownload` | YouTube URL | `!ytdownload https://www.youtube.com/watch?v=...` | Download and upload video to Discord |

## ⚠️ Operational Notes

### Music Commands

- **User Location:** You must be connected to a voice channel for music commands to work
- **Bot Permissions:** Bot needs "Speak" and "Connect" permissions in the server
- **Quality:** Audio is streamed directly via yt-dlp/ffmpeg (real-time encoding)
- **Download Limits:** `!ytdownload` respects Discord's per-user upload limit (~8 MB for non-Nitro accounts)

### Dependencies

- **Missing yt-dlp:** Music commands return helpful error messages instead of failing silently
- **Missing ffmpeg:** Audio processing returns an error; check system installation
- **Network Issues:** Bot auto-reconnects if Discord connection is lost (with exponential backoff)

### Token Validation

- **Invalid Token (401):** The bot detects `401 Unauthorized` and exits immediately instead of reconnecting in a loop
  - Check that your token is correctly copied from Discord Developer Portal
  - Verify the token hasn't expired or been regenerated
  - Ensure the bot is actually invited to your server
- **Valid Token:** Connection succeeds silently

## 🗂️ Project Structure

```
.
├── Cargo.toml                  # Rust project manifest
├── Cargo.lock                  # Dependency lock file
├── .env                        # Discord token (not in git)
├── README.md                   # This file
│
├── src/
│   ├── main.rs                 # Entry point, connection handling
│   ├── commands/
│   │   ├── mod.rs              # Commands module export
│   │   ├── ping.rs             # !ping command  
│   │   └── music.rs            # !play, !yt, !stop, !leave, !ytdownload
│   └── events/
│       ├── mod.rs              # Events module export
│       └── ready.rs            # Ready event handler
│
└── deploy/
    └── discord-bot.service     # systemd service configuration
```

## 🐛 Troubleshooting

| Issue | Cause | Solution |
|-------|-------|----------|
| `Bot not responding to commands` | Prefix not recognized or bot has no message content intents | Verify `MESSAGE CONTENT INTENT` is enabled in Developer Portal |
| `!play command fails` | yt-dlp not installed or URL is invalid | Run `pip3 install -U yt-dlp` and verify URL is a valid YouTube link |
| `No audio in voice channel` | ffmpeg not installed or missing permissions | Install ffmpeg: `sudo apt install ffmpeg`, verify "Speak" permission |
| `Bot disconnects randomly` | Network instability or token issues | Check internet connection; if token error, restart the bot |
| `systemd service fails to start` | Wrong paths or Missing `.env` | Verify paths in service file, ensure `.env` exists in home directory |
| `Binary size too large` | Debug build includes symbols | Always use `cargo build --release` for Raspberry Pi |
| `Slow compilation` | Raspberry Pi 3B+ building from source | Consider cross-compiling or using pre-built binaries |

### Debug Mode

To see detailed logs during development:

```bash
RUST_LOG=debug cargo run
```

For systemd service, create an override:

```bash
sudo systemctl edit discord-bot

# Add:
[Service]
Environment="RUST_LOG=debug"
```

Then restart: `sudo systemctl restart discord-bot` and check logs with `journalctl -u discord-bot -f`

## ⚡ Performance Optimization

### Binary Size & Speed

The Cargo.toml includes optimizations:

```toml
[profile.release]
lto = true              # Link-time optimization
strip = true            # Remove debugging symbols
codegen-units = 1       # Slower build, faster binary
```

**Result:** ~20 MB optimized binary on Raspberry Pi 4, startup < 2 seconds

### Memory Usage

- Base memory footprint: ~30-50 MB
- Per-connection overhead: ~5 MB
- Audio streaming: ~20-50 MB depending on bitrate

### CPU Usage

- Idle (connected): ~1-2% CPU
- Streaming audio: ~5-15% CPU (Raspberry Pi 4)
- During ytdownload: Up to 30% (single-threaded ffmpeg)

## 🔐 Security

- ✅ Token stored only in `.env` (in `.gitignore`)
- ✅ No secrets logged or exposed
- ✅ Binary audit ready (run `cargo audit`)
- ✅ Dependencies are locked in `Cargo.lock`
- ✅ Scheduled dependency updates recommended

Check for security advisories:

```bash
cargo audit
```

## 📚 Resources

- [Rust Book](https://doc.rust-lang.org/book/)
- [Serenity Docs](https://docs.rs/serenity/latest/serenity/)
- [Songbird (Voice) Docs](https://docs.rs/songbird/latest/songbird/)
- [Discord.py Intents Reference](https://discordpy.readthedocs.io/en/stable/intents.html) (applies to all clients)
- [systemd Best Practices](https://www.freedesktop.org/software/systemd/man/systemd.service.html)

## 📝 License

This project is provided as-is for educational and personal use on Raspberry Pi and Linux systems.

## 🤝 Contributing

Feel free to fork and submit improvements. Common contributions:

- Additional commands (music queue, playlist support)
- Improved error handling
- Performance benchmarks
- Docker support

## 📍 Repository

- **GitHub:** https://github.com/devcepeda/raspberry-discord-with-rust
- **Issues:** Report bugs or feature requests on GitHub Issues
- **Discussions:** Ask questions in GitHub Discussions

URL objetivo del repositorio:

```text
https://github.com/devcepeda/raspberry-discord-with-rust
```

## Siguiente paso recomendado

1. Verificar que el bot tenga intents habilitados en Discord Developer Portal.
2. Construir con `cargo build --release`.
3. Levantarlo con `cargo run` o como servicio systemd.