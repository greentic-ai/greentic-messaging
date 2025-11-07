const DEFAULT_PROPS_MSG =
  "Support for defaultProps will be removed from function components";

function filter(method: "warn" | "error") {
  const orig = console[method].bind(console);
  return (...args: unknown[]) => {
    const msg = String(args[0] ?? "");
    if (msg.includes(DEFAULT_PROPS_MSG)) {
      return;
    }
    orig(...args);
  };
}

if (import.meta.env.DEV && !(window as { __consoleFiltered__?: boolean }).__consoleFiltered__) {
  console.warn = filter("warn");
  console.error = filter("error");
  (window as { __consoleFiltered__?: boolean }).__consoleFiltered__ = true;
}
