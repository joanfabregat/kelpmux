# Native client boundary

Kelpmux owns long-running PTYs and terminal interpretation; clients own presentation. Grid and tabs are alternate views of the same stable pane IDs. Dialogs belong to a frontend and do not create another agent terminal. The terminal frontend currently shares workspace presentation with its controller; this is an implementation of a client, not the eventual native-client layout contract.

## Transport direction

A future desktop, iPad or iPhone client should authenticate through SSH and open a dedicated command channel, for example `kelpmux bridge --workspace main`. This bridge is a planned command, not a command available in this beta. It will forward versioned, length-prefixed Kelpmux messages between SSH standard input/output and the existing private Unix socket. One SSH connection can carry all panes: each terminal message carries a stable pane ID, allowing the native client to demultiplex updates into its own views. SSH handles authentication, encryption and flow control; Kelpmux needs no additional listening network port.

The native channel must use an SSH command request without an outer PTY. Outer PTYs can translate bytes and merge diagnostics into the protocol. Existing terminal clients continue using an interactive SSH PTY and the ANSI frontend. Shell startup scripts must not print anything into a native protocol channel. Diagnostics go to stderr, and the bridge must forward bounded messages, check peer ownership and handle disconnects without terminating terminals.

## Independent terminal data

The beta provides `InspectPane { pane }` and `PaneState { pane, screen }` as an initial presentation-independent seam. A screen contains terminal cells with colors and attributes, cursor position and input modes. It is independent of composed terminal UI headers, tabs and footer cells. Inspection does not change control ownership, active pane or terminal dimensions. Unknown pane IDs return an error.

A complete native-client protocol will add subscriptions, per-pane revisions and cell deltas, pane-addressed input/mouse/resize requests, and paginated history/selection/search. All mutating requests must follow explicit controller ownership, size and input limits. Clients reconnect by requesting a snapshot and then subscribing from its revision. Slow consumers receive bounded queues and resynchronization rather than unbounded retained updates. A client can maintain one subscription for each terminal while rendering only the current tab.

Decoded cells are the first implementation target. Alacritty on the server remains the authoritative emulator and answers application terminal queries. Sending raw VT bytes into another emulator would require an explicit policy that prevents both emulators from answering the same query. Native scrolling means clients request history ranges and render them locally; it does not require mirroring the terminal frontend's scrolling position.

## Responsive presentation

The terminal frontend selects top, left or compact bottom tab navigation from available character dimensions and permits an explicit override. This is a geometry decision rather than device detection. A native client can choose its own adaptive UI from pixel dimensions and platform conventions, retain native scrolling and accessibility, and keep processes alive when views change. These client layouts must never implicitly close hidden terminals.
