# Kiri for Windows

This package installs the native Windows launcher and the matching Linux runtime into an existing,
supported WSL2 distribution. It also installs `bubblewrap` inside that distribution.

```powershell
npm install --global @kiri-ai/cli
# or
bun add --global --trust @kiri-ai/cli
```

Bun blocks dependency lifecycle scripts unless the package is explicitly trusted; `--trust` is required
because the installer is the package's `postinstall` script.

Open a new terminal after installation, then run `kiri wsl status`. Supported distributions are Ubuntu
22.04/24.04/26.04 and Debian 12/13. If none is installed, run
`wsl --install -d Ubuntu-24.04`, finish its first-launch setup, and retry.
