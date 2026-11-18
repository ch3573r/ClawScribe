"use client";

import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/label";
import { PageSection } from "@/components/ui/page-section";
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";

interface Access {
  enabled: boolean;
  command: string;
}

export function LocalMcpSettings() {
  const [access, setAccess] = useState<Access | null>(null);
  const [busy, setBusy] = useState(true);
  const [error, setError] = useState("");
  const [copied, setCopied] = useState(false);
  const request = useRef(0);
  const pending = useRef(false);

  async function refresh(enabled?: boolean) {
    if (pending.current) return;
    pending.current = true;
    const id = ++request.current;
    setBusy(true);
    setError("");
    setCopied(false);
    try {
      const result = enabled === undefined
        ? await invoke<Access>("get_mcp_access")
        : await invoke<Access>("set_mcp_access", { enabled });
      if (id === request.current) setAccess(result);
    } catch {
      if (id === request.current) {
        setAccess(null);
        setError("Could not verify local assistant access. Retry to check the current setting.");
      }
    } finally {
      if (id === request.current) {
        pending.current = false;
        setBusy(false);
      }
    }
  }

  useEffect(() => {
    void refresh();
    return () => { request.current++; pending.current = false; };
  }, []);

  const config = access?.enabled
    ? JSON.stringify({ mcpServers: { clawscribe: { command: access.command, args: ["--mcp-stdio"] } } }, null, 2)
    : "";

  async function copy() {
    setError("");
    try {
      await navigator.clipboard.writeText(config);
      setCopied(true);
    } catch {
      setError("Select the configuration below and copy it manually.");
    }
  }

  return (
    <PageSection title="Local assistant access" description="Connect an MCP-compatible assistant to your saved meeting library.">
      <div className="space-y-4 p-5">
        <div className="flex items-start justify-between gap-4">
          <div>
            <Label htmlFor="mcp-access">Allow read-only access</Label>
            <p className="mt-1 text-sm text-muted-foreground">
              Local apps running as you can read meeting titles, summaries, transcripts and saved audio paths,
              even while ClawScribe is closed. A connected assistant may send what it reads to its provider.
            </p>
          </div>
          <Switch id="mcp-access" checked={access?.enabled ?? false} disabled={busy || !access}
            onCheckedChange={value => void refresh(value)} />
        </div>
        {busy && <p role="status" className="text-sm text-muted-foreground">Checking access…</p>}
        {config && <div className="space-y-2">
          <Label htmlFor="mcp-client-config">Assistant configuration</Label>
          <Textarea id="mcp-client-config" value={config} readOnly rows={7} className="font-mono text-xs"
            onFocus={event => event.target.select()} />
          <Button variant="outline" size="sm" disabled={busy} onClick={() => void copy()}>
            {copied ? "Copied" : "Copy configuration"}
          </Button>
        </div>}
        {error && <p role="alert" className="text-sm text-destructive">{error}</p>}
        {!access && !busy && <Button size="sm" variant="outline" onClick={() => void refresh()}>Retry</Button>}
      </div>
    </PageSection>
  );
}
