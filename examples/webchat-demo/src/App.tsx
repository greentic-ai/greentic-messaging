import React, { useCallback, useEffect, useMemo, useRef, useState } from "react";
import ReactWebChat, { createDirectLine } from "botframework-webchat";

type TokenResponse = {
  token: string;
  expires_in: number;
};

type Status = "idle" | "loading" | "ready" | "error";

const envVars = import.meta.env as Record<string, string | undefined>;
const IS_DEV = import.meta.env.DEV;

const WEBCHAT_BASE_URL = (envVars.WEBCHAT_BASE_URL ?? envVars.VITE_WEBCHAT_BASE_URL ?? "")
  .trim()
  .replace(/\/$/, "");
const DIRECT_LINE_ROUTE = `/v3/directline`;
const WEBCHAT_DIRECT_LINE_DOMAIN = (
  envVars.WEBCHAT_DIRECTLINE_DOMAIN ??
  envVars.VITE_WEBCHAT_DIRECTLINE_DOMAIN ??
  DIRECT_LINE_ROUTE
)
  .trim()
  .replace(/\/$/, "") || DIRECT_LINE_ROUTE;
const WEBCHAT_ENV = (envVars.WEBCHAT_ENV ?? envVars.VITE_WEBCHAT_ENV ?? "dev").trim() || "dev";
const WEBCHAT_TENANT = (envVars.WEBCHAT_TENANT ?? envVars.VITE_WEBCHAT_TENANT ?? "demo").trim() || "demo";
const WEBCHAT_TEAM = (envVars.WEBCHAT_TEAM ?? envVars.VITE_WEBCHAT_TEAM ?? "").trim();
const WEBCHAT_USER_ID =
  (envVars.WEBCHAT_USER_ID ?? envVars.VITE_WEBCHAT_USER_ID ?? "greentic-demo-user").trim() ||
  "greentic-demo-user";

if (IS_DEV) {
  console.info("[webchat-demo] configuration", {
    baseUrl: WEBCHAT_BASE_URL || "(relative)",
    directLineDomain: WEBCHAT_DIRECT_LINE_DOMAIN || DIRECT_LINE_ROUTE,
    env: WEBCHAT_ENV,
    tenant: WEBCHAT_TENANT,
    team: WEBCHAT_TEAM || "(none)",
    userId: WEBCHAT_USER_ID,
  });
}

function resolveUrl(path: string): string {
  if (WEBCHAT_BASE_URL) {
    return `${WEBCHAT_BASE_URL}${path}`;
  }
  return path;
}

async function postJSON<T>(
  path: string,
  body?: unknown,
  extraHeaders?: Record<string, string>,
): Promise<T> {
  const headers: Record<string, string> = {
    "content-type": "application/json",
    ...extraHeaders,
  };
  const url = resolveUrl(path);
  if (IS_DEV) {
    console.info("[webchat-demo] POST", url, body ?? {});
  }

  const response = await fetch(url, {
    method: "POST",
    headers,
    body: JSON.stringify(body ?? {}),
  });

  if (IS_DEV) {
    console.info("[webchat-demo] response", response.status, response.statusText, "for", url);
  }

  if (!response.ok) {
    const text = await response.text();
    if (IS_DEV) {
      console.error("[webchat-demo] request failed", url, text);
    }
    throw new Error(`${response.status} ${response.statusText}: ${text}`);
  }

  return response.json() as Promise<T>;
}

export default function App(): JSX.Element {
  const [status, setStatus] = useState<Status>("idle");
  const [error, setError] = useState<string | null>(null);
  const [directLine, setDirectLine] = useState<ReturnType<typeof createDirectLine>>();
  const [expiresIn, setExpiresIn] = useState<number | null>(null);
  const startedRef = useRef(false);

  const refreshSession = useCallback(async () => {
    setStatus("loading");
    setError(null);
    if (IS_DEV) {
      console.info("[webchat-demo] refreshing session", {
        env: WEBCHAT_ENV,
        tenant: WEBCHAT_TENANT,
        team: WEBCHAT_TEAM || "-",
      });
    }
    try {
      const query = new URLSearchParams({
        env: WEBCHAT_ENV,
        tenant: WEBCHAT_TENANT,
      });
      if (WEBCHAT_TEAM) {
        query.set("team", WEBCHAT_TEAM);
      }

      const tokenResponse = await postJSON<TokenResponse>(`${DIRECT_LINE_ROUTE}/tokens/generate?${query.toString()}`, {
        user: { id: WEBCHAT_USER_ID },
      });
      setExpiresIn(tokenResponse.expires_in ?? null);
      if (IS_DEV) {
        console.info("[webchat-demo] token issued", { expires_in: tokenResponse.expires_in });
      }

      const dl = createDirectLine({
        token: tokenResponse.token,
        domain: WEBCHAT_DIRECT_LINE_DOMAIN || DIRECT_LINE_ROUTE,
        webSocket: false,
      });
      if (IS_DEV) {
        dl.connectionStatus$.subscribe((status) => {
          console.info("[webchat-demo] directline status", status);
        });
        dl.activity$.subscribe((activity) => {
          console.info("[webchat-demo] directline activity", activity.type);
        });
      }
      setDirectLine(dl);
      setStatus("ready");
    } catch (err) {
      console.error("failed to initialise web chat session", err);
      setError(err instanceof Error ? err.message : String(err));
      setStatus("error");
    }
  }, []);

  useEffect(() => {
    if (startedRef.current) {
      return;
    }
    startedRef.current = true;
    void refreshSession();
  }, [refreshSession]);

  const infoBlock = useMemo(
    () => (
      <section className="info">
        <h1>Greentic Web Chat Demo</h1>
        <p>
          This demo uses the standalone Direct Line surface from{" "}
          <code>gsm_core::platforms::webchat</code> to bridge Microsoft Bot Framework Web
          Chat to Greentic NG. Configure the environment via the{" "}
          <code>VITE_WEBCHAT_*</code> variables in <code>.env.local</code>.
        </p>
        <dl>
          <div>
            <dt>Environment</dt>
            <dd>{WEBCHAT_ENV}</dd>
          </div>
          <div>
            <dt>Tenant</dt>
            <dd>{WEBCHAT_TENANT}</dd>
          </div>
          {WEBCHAT_TEAM && (
            <div>
              <dt>Team</dt>
              <dd>{WEBCHAT_TEAM}</dd>
            </div>
          )}
          {expiresIn !== null && (
            <div>
              <dt>Token expires (seconds)</dt>
              <dd>{expiresIn}</dd>
            </div>
          )}
        </dl>
        <div className="actions">
          <button onClick={() => refreshSession()} disabled={status === "loading"}>
            New conversation
          </button>
        </div>
      </section>
    ),
    [expiresIn, refreshSession],
  );

  return (
    <div className="layout">
      {infoBlock}
      <section className="chat">
        {status === "loading" && <p className="status">Starting Direct Line…</p>}
        {status === "error" && (
          <div className="status error">
            <p>Failed to start conversation.</p>
            {error && <pre>{error}</pre>}
            <button onClick={() => refreshSession()}>Try again</button>
          </div>
        )}
        {status === "ready" && directLine ? (
          <ReactWebChat directLine={directLine} userID="greentic-demo-user" />
        ) : null}
      </section>
    </div>
  );
}
