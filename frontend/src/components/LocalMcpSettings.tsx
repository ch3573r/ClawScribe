"use client";

import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { AlertCircle } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { SettingsCard, SettingsRow } from "@/components/ui/settings-layout";
import { NoteRow } from "@/components/settings/settings-parts";
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
  const [client, setClient] = useState("json");
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

  let config = "";
  if (access?.enabled) {
    const server = { command: access.command, args: ["--mcp-stdio"] };
    config = client === "codex"
      ? `[mcp_servers.clawscribe]\ncommand = ${JSON.stringify(server.command).replace(/\u007f/g, "\\u007f")}\nargs = ${JSON.stringify(server.args)}`
      : JSON.stringify({ mcpServers: { clawscribe: server } }, null, 2);
  }

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
    <>
      <SettingsCard>
        <SettingsRow
          title="Allow read-only access"
          htmlFor="mcp-access"
          description="Local apps running as you can read meeting titles, summaries, transcripts and audio paths, also while ClawScribe is closed"
        >
          {busy && <span role="status" className="text-[13px] text-muted-foreground">Checking access…</span>}
          {!access && !busy && <Button size="sm" variant="outline" onClick={() => void refresh()}>Retry</Button>}
          <Switch id="mcp-access" checked={access?.enabled ?? false} disabled={busy || !access}
            onCheckedChange={value => void refresh(value)} />
        </SettingsRow>
        {config && <div className="grid gap-2 px-5 py-4">
          <Label htmlFor="mcp-client">Client format</Label>
          <Select value={client} onValueChange={value => { setClient(value); setCopied(false); }}>
            <SelectTrigger id="mcp-client"><SelectValue /></SelectTrigger>
            <SelectContent>
              <SelectItem value="json">Claude Desktop and compatible clients (JSON)</SelectItem>
              <SelectItem value="codex">Codex (TOML)</SelectItem>
            </SelectContent>
          </Select>
          <Label htmlFor="mcp-client-config">Assistant configuration</Label>
          <Textarea id="mcp-client-config" value={config} readOnly rows={7} className="font-mono text-xs"
            onFocus={event => event.target.select()} />
          <div>
            <Button variant="outline" size="sm" disabled={busy} onClick={() => void copy()}>
              {copied ? "Copied" : "Copy configuration"}
            </Button>
          </div>
        </div>}
        {error && <NoteRow tone="error">{error}</NoteRow>}
      </SettingsCard>
      <div className="flex items-start gap-3 rounded-lg bg-warning px-4 py-3.5 text-[13px] leading-5 text-warning-foreground">
        <AlertCircle aria-hidden="true" className="mt-0.5 h-4 w-4 shrink-0" />
        <span>A connected assistant may send what it reads to its own provider. This is the only setting that lets other apps read your library.</span>
      </div>
    </>
  );
}
