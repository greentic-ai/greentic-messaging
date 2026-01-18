# Legacy feature gate

The legacy messaging stack remains in-tree only for transition work. It is feature-gated and does
not compile or run in default builds. Enable it explicitly with the `legacy` Cargo feature when you
need to build legacy binaries or compatibility paths. The legacy code will be removed once the
transition is complete.
