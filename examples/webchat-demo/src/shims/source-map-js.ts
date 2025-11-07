// Minimal browser shim to satisfy packages that import `source-map(-js)` APIs.
// The demo never inspects source maps at runtime, so each method is a no-op.

export class SourceMapConsumer {
  constructor(_map: unknown) {
    // no-op
  }
  public static async initialize(): Promise<void> {
    return;
  }
  public async originalPositionFor(): Promise<Record<string, never>> {
    return {};
  }
  public destroy(): void {
    // no-op
  }
}

export class SourceMapGenerator {
  public toString(): string {
    return "";
  }
}

export default {
  SourceMapConsumer,
  SourceMapGenerator,
};
