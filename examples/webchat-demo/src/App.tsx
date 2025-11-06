import React, { useCallback, useEffect, useMemo, useState } from "react";
import ReactWebChat, { createDirectLine } from "botframework-webchat";

type TokenResponse = {
  token: string;
  expires_in: number;
};

type ConversationResponse = {
  token: string;
  conversationId: string;
  streamUrl?: string;
};

type Status = "idle" | "loading" | "ready" | "error";

const WEBCHAT_BASE_URL = (import.meta.env.VITE_WEBCHAT_BASE_URL ?? "").replace(/\/$/, "");
const WEBCHAT_DIRECT_LINE_DOMAIN =
  (import.meta.env.VITE_WEBCHAT_DIRECTLINE_DOMAIN as string | undefined)?.replace(/\/$/, "") ??
  "https://localhost:8080/v3/directline";
const WEBCHAT_ENV = import.meta.env.VITE_WEBCHAT_ENV || "dev";
const WEBCHAT_TENANT = import.meta.env.VITE_WEBCHAT_TENANT || "demo";
const WEBCHAT_TEAM = import.meta.env.VITE_WEBCHAT_TEAM || "";

const DIRECT_LINE_ROUTE = `/v3/directline`;

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

  const response = await fetch(resolveUrl(path), {
    method: "POST",
    headers,
    body: body === undefined ? undefined : JSON.stringify(body),
  });

  if (!response.ok) {
    const text = await response.text();
    throw new Error(`${response.status} ${response.statusText}: ${text}`);
  }

  return response.json() as Promise<T>;
}

export default function App(): JSX.Element {
  const [status, setStatus] = useState<Status>("idle");
  const [error, setError] = useState<string | null>(null);
  const [directLine, setDirectLine] = useState<ReturnType<typeof createDirectLine>>();
  const [expiresIn, setExpiresIn] = useState<number | null>(null);

  const refreshSession = useCallback(async () => {
    setStatus("loading");
    setError(null);
    try {
      const query = new URLSearchParams({
        env: WEBCHAT_ENV,
        tenant: WEBCHAT_TENANT,
      });
      if (WEBCHAT_TEAM) {
        query.set("team", WEBCHAT_TEAM);
      }

      const tokenResponse = await postJSON<TokenResponse>(
        `${DIRECT_LINE_ROUTE}/tokens/generate?${query.toString()}`,
      );
      setExpiresIn(tokenResponse.expires_in ?? null);

      const conversationResponse = await postJSON<ConversationResponse>(
        `${DIRECT_LINE_ROUTE}/conversations`,
        undefined,
        { Authorization: `Bearer ${tokenResponse.token}` },
      );

      const dl = createDirectLine({
        token: conversationResponse.token,
        domain: WEBCHAT_DIRECT_LINE_DOMAIN,
      });
      setDirectLine(dl);
      setStatus("ready");
    } catch (err) {
      console.error("failed to initialise web chat session", err);
      setError(err instanceof Error ? err.message : String(err));
      setStatus("error");
    }
  }, []);

  useEffect(() => {
    void refreshSession();
  }, [refreshSession]);

  const infoBlock = useMemo(
    () => (
      <section className="info">
        <h1>Greentic Web Chat Demo</h1>
        <p>
          This demo uses the standalone Direct Line surface from{" "}
          <code>providers/webchat</code> to bridge Microsoft Bot Framework Web Chat to
          Greentic NG. Configure the environment via the <code>VITE_WEBCHAT_*</code>{" "}
          variables in <code>.env.local</code>.
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
