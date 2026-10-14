# Synthetic terminal fixtures

These JSON strings contain hand-authored ANSI/VT sequences, never captured user sessions or agent transcripts. `agent-redraw` models streaming status, input backgrounds, Unicode and cursor queries. `alternate-tools` models a full-screen tool, primary-screen restoration, editing, mouse modes and dynamic color queries. Tests compare whole and fragmented delivery and replay the resulting terminal screen.
