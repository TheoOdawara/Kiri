# Starting the daemon

The first client finds no daemon and starts one. Milestone: M1.
Requirements: [FR-CNT-02](../../requirements/functional/continuity.md#fr-cnt-02),
[NFR-SEC-02](../../requirements/non-functional/security.md#nfr-sec-02).

1. **kiri** tries to connect to the socket or named pipe, and fails.
2. **kiri** spawns its own executable as `kiri daemon`, detached from the terminal. The daemon inherits the client's environment.
3. The **daemon** takes the exclusive lock on `~/.kiri/daemon.lock`. A daemon that loses the lock exits, so two clients starting together end with one daemon.
4. The **daemon** removes any leftover socket, binds the endpoint with owner-only access, and reads the environment once.
5. **kiri** retries every 50 ms for up to 5 seconds, connects, and sends `initialize`.
6. On Linux and macOS the **daemon** checks the peer's uid. On Windows **kiri** checks the pipe's owner.
7. Until [#3](https://github.com/TheoOdawara/Kiri/issues/3), the **daemon** exits when its last client leaves, or 5 seconds after starting when no client connected.
