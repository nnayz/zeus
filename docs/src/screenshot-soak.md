# Screenshot soak test

The default Rust test suite uses a fake capturer and never requests macOS
Screen Recording permission. To exercise a real Zeus window deliberately, run
this from `zeus/` while the Zeus app has an open window:

```sh
ZEUS_SCREENSHOT_SOAK=1 cargo test -p zeus-engine real_window_soak_is_explicitly_opt_in
```

macOS may ask for Screen Recording permission for Zeus. The test captures one
720p JPEG in memory, stops the stream immediately, and writes no screenshot
file, so there is no artifact cleanup. Remove Zeus under **System Settings →
Privacy & Security → Screen Recording** if you also want to clear the grant.
