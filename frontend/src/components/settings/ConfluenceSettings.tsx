"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import { Loader2 } from "lucide-react";
import { UnencryptedHttpOptIn } from "@/components/UnencryptedHttpOptIn";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { SettingsCard, SettingsFooter, SettingsRow, SettingsStack, StatusText } from "@/components/ui/settings-layout";
import { getExportDestinations, setExportDestinations, type ConfluenceExportMode } from "@/lib/exportDestinations";
import { confluenceExportService, type ConfluenceConnectionStatus } from "@/services/confluenceExportService";
import { confluenceDot } from "./useConnectionStatus";

const FIELD = "w-[26rem] max-w-full";

type StatusObserver = (status: ReturnType<typeof confluenceDot>) => void;

export function ConfluenceCard({ onStatusChange }: { onStatusChange?: StatusObserver } = {}) {
  const saved = getExportDestinations();
  const [mode, setMode] = useState<ConfluenceExportMode>(saved.confluenceMode ?? "draft");
  const [createUrl, setCreateUrl] = useState(saved.confluenceCreateUrl ?? "");
  const [openAfterCopy, setOpenAfterCopy] = useState(saved.confluenceOpenAfterCopy ?? true);
  const [baseUrl, setBaseUrl] = useState(saved.confluenceBaseUrl ?? "");
  const [spaceKey, setSpaceKey] = useState(saved.confluenceSpaceKey ?? "");
  const [parentId, setParentId] = useState(saved.confluenceParentId ?? "");
  const [allowUnencrypted, setAllowUnencrypted] = useState(false);
  const httpOptionRevision = useRef(0);
  const [patInput, setPatInput] = useState("");
  const [status, setStatus] = useState<ConfluenceConnectionStatus | null>(null);
  const [connectionTestSucceeded, setConnectionTestSucceeded] = useState(false);
  const [testFailed, setTestFailed] = useState(false);
  const [busy, setBusy] = useState<"save" | "clear" | "test" | null>(null);
  const invalidateConnectionTest = () => {
    httpOptionRevision.current += 1;
    setConnectionTestSucceeded(false);
    setTestFailed(false);
  };
  const trimmedUrl = createUrl.trim();
  const trimmedBaseUrl = baseUrl.trim();
  useEffect(() => {
    onStatusChange?.(confluenceDot(mode, !!trimmedUrl, status, testFailed));
  }, [mode, trimmedUrl, status, testFailed, onStatusChange]);
  useEffect(() => {
    let active = true;
    const revision = ++httpOptionRevision.current;
    setAllowUnencrypted(false);
    confluenceExportService.settingsStatus(trimmedBaseUrl).then(value => {
      if (active && revision === httpOptionRevision.current) {
        setStatus(value);
        setAllowUnencrypted(value.allowUnencrypted);
        if (!trimmedBaseUrl && value.baseUrl) setBaseUrl(value.baseUrl);
      }
    }).catch(error => {
      if (active && revision === httpOptionRevision.current) setStatus({
        tokenConfigured: false,
        allowUnencrypted: false,
        reachable: false,
        userDisplayName: null,
        message: error instanceof Error ? error.message : String(error),
      });
    });
    return () => { active = false; };
  }, [trimmedBaseUrl]);
  const trimmedSpaceKey = spaceKey.trim();
  const trimmedParentId = parentId.trim();

  useEffect(() => {
    setExportDestinations({
      confluenceMode: mode,
      confluenceCreateUrl: trimmedUrl || undefined,
      confluenceOpenAfterCopy: openAfterCopy,
      confluenceBaseUrl: trimmedBaseUrl || undefined,
      confluenceSpaceKey: trimmedSpaceKey || undefined,
      confluenceParentId: trimmedParentId || undefined,
    });
  }, [mode, openAfterCopy, trimmedBaseUrl, trimmedParentId, trimmedSpaceKey, trimmedUrl]);

  const testConnection = useCallback(async () => {
    const revision = ++httpOptionRevision.current;
    setConnectionTestSucceeded(false);
    setTestFailed(false);
    if (!trimmedBaseUrl) {
      setStatus({
        tokenConfigured: false,
        allowUnencrypted,
        reachable: false,
        userDisplayName: null,
        message: "Enter a Confluence base URL first.",
      });
      return;
    }

    setBusy("test");
    try {
      const value = await confluenceExportService.connectionStatus(trimmedBaseUrl);
      if (revision === httpOptionRevision.current) {
        setStatus(value);
        setAllowUnencrypted(value.allowUnencrypted);
        setConnectionTestSucceeded(value.reachable);
        setTestFailed(!value.reachable);
      }
    } catch (e) {
      setTestFailed(true);
      setStatus({
        tokenConfigured: true,
        allowUnencrypted,
        reachable: false,
        userDisplayName: null,
        message: e instanceof Error ? e.message : String(e),
      });
    } finally {
      setBusy(null);
    }
  }, [allowUnencrypted, trimmedBaseUrl]);

  const savePat = useCallback(async () => {
    const revision = ++httpOptionRevision.current;
    setConnectionTestSucceeded(false);
    const pat = patInput.trim();
    if (!pat) {
      setStatus({
        tokenConfigured: false,
        allowUnencrypted,
        reachable: false,
        userDisplayName: null,
        message: "Paste a Confluence personal access token first.",
      });
      return;
    }

    setBusy("save");
    try {
      await confluenceExportService.savePat(pat, trimmedBaseUrl, allowUnencrypted);
      setPatInput("");
      if (trimmedBaseUrl) {
        const value = await confluenceExportService.connectionStatus(trimmedBaseUrl);
        if (revision === httpOptionRevision.current) {
          setStatus(value);
          setAllowUnencrypted(value.allowUnencrypted);
        }
      } else {
        setStatus({
          tokenConfigured: true,
          allowUnencrypted,
          reachable: false,
          userDisplayName: null,
          message: "Token saved. Add a base URL, then test the connection.",
        });
      }
    } catch (e) {
      setStatus({
        tokenConfigured: false,
        allowUnencrypted,
        reachable: false,
        userDisplayName: null,
        message: e instanceof Error ? e.message : String(e),
      });
    } finally {
      setBusy(null);
    }
  }, [allowUnencrypted, patInput, trimmedBaseUrl]);

  const clearPat = useCallback(async () => {
    httpOptionRevision.current += 1;
    setConnectionTestSucceeded(false);
    setBusy("clear");
    try {
      await confluenceExportService.clearPat();
      setPatInput("");
      setAllowUnencrypted(false);
      setStatus({
        tokenConfigured: false,
        allowUnencrypted: false,
        reachable: false,
        userDisplayName: null,
        message: "Saved Confluence token cleared.",
      });
    } catch (e) {
      setStatus({
        tokenConfigured: true,
        allowUnencrypted,
        reachable: false,
        userDisplayName: null,
        message: e instanceof Error ? e.message : String(e),
      });
    } finally {
      setBusy(null);
    }
  }, [allowUnencrypted]);

  const changeMode = (next: string) => {
    invalidateConnectionTest();
    setMode(next === "rest" ? "rest" : "draft");
  };

  return (
    <SettingsCard>
      <SettingsRow
        title="Method"
        description={mode === "draft"
          ? "Browser draft copies the notes and opens Confluence in your signed-in browser. No token is stored."
          : "Direct REST creates pages on a self-hosted Confluence Server or Data Center you can reach, for example over a VPN. It needs a personal access token that can create pages in the space."}
      >
        <Tabs value={mode} onValueChange={changeMode}>
          <TabsList aria-label="Method">
            <TabsTrigger value="draft">Browser draft</TabsTrigger>
            <TabsTrigger value="rest">Direct REST</TabsTrigger>
          </TabsList>
        </Tabs>
      </SettingsRow>

      {mode === "draft" ? (
        <>
          <SettingsRow
            title="Create page URL"
            htmlFor="confluence-create-url"
            description="Opens in your browser session, so SSO and App Proxy work"
          >
            <Input
              id="confluence-create-url"
              type="url"
              value={createUrl}
              onChange={(e) => setCreateUrl(e.target.value)}
              placeholder="https://confluence.example.com/pages/createpage.action?spaceKey=TEAM"
              className={`${FIELD} font-mono text-xs`}
            />
          </SettingsRow>
          <SettingsRow
            title="Open Confluence after copying"
            htmlFor="confluence-open-after-copy"
            description="Paste into the editor and save under the space you want"
          >
            <Switch id="confluence-open-after-copy" checked={openAfterCopy} onCheckedChange={() => setOpenAfterCopy((v) => !v)} />
          </SettingsRow>
        </>
      ) : (
        <>
          <SettingsRow title="Base URL" htmlFor="confluence-base-url">
            <Input
              id="confluence-base-url"
              type="url"
              value={baseUrl}
              onChange={(e) => { invalidateConnectionTest(); setBaseUrl(e.target.value); }}
              placeholder="https://confluence.example.com/confluence"
              className={`${FIELD} font-mono text-xs`}
            />
          </SettingsRow>
          <div className="px-5 empty:hidden [&>*]:py-3">
            <UnencryptedHttpOptIn destinationProblem={status?.destinationProblem} urls={[baseUrl]} checked={allowUnencrypted} onChange={value => {
              invalidateConnectionTest();
              setAllowUnencrypted(value);
            }} />
          </div>
          <SettingsRow title="Space key" htmlFor="confluence-space-key">
            <Input
              id="confluence-space-key"
              value={spaceKey}
              onChange={(e) => { invalidateConnectionTest(); setSpaceKey(e.target.value); }}
              placeholder="TEAM"
              className="w-48 max-w-full font-mono text-xs"
            />
          </SettingsRow>
          <SettingsRow title="Parent page ID" htmlFor="confluence-parent-id" description="Optional">
            <Input
              id="confluence-parent-id"
              value={parentId}
              onChange={(e) => { invalidateConnectionTest(); setParentId(e.target.value); }}
              placeholder="123456789"
              className="w-48 max-w-full font-mono text-xs"
            />
          </SettingsRow>
          <SettingsRow
            title="Personal access token"
            htmlFor="confluence-token"
            description={status?.tokenConfigured ? "A token is saved in the OS credential store" : "Saved in the OS credential store, never in exported content"}
          >
            <Input
              id="confluence-token"
              type="password"
              autoComplete="off"
              value={patInput}
              onChange={(e) => { invalidateConnectionTest(); setPatInput(e.target.value); }}
              placeholder="Paste token"
              className="w-56 max-w-full"
            />
            <Button type="button" variant="outline" size="sm" onClick={savePat} disabled={busy !== null || !patInput.trim()}>
              {busy === "save" && <Loader2 aria-hidden="true" className="animate-spin" />}Save token
            </Button>
            <Button type="button" variant="ghost" size="sm" onClick={clearPat} disabled={busy !== null}>
              {busy === "clear" && <Loader2 aria-hidden="true" className="animate-spin" />}Clear
            </Button>
          </SettingsRow>
          <SettingsFooter
            actions={
              <Button type="button" variant="outline" size="sm" onClick={testConnection} disabled={busy !== null || !trimmedBaseUrl}>
                {busy === "test" && <Loader2 aria-hidden="true" className="animate-spin" />}Test connection
              </Button>
            }
          >
            {busy === "test" ? <StatusText tone="active">Testing…</StatusText>
              : connectionTestSucceeded ? <StatusText tone="ok"><span role="status">Connection works</span></StatusText>
              : status && !status.reachable && status.message ? <StatusText tone={testFailed ? "warning" : "off"} className="whitespace-normal">{status.message}</StatusText>
              : <StatusText tone="off">Not tested</StatusText>}
            <span>Direct export also needs the base URL and space key</span>
          </SettingsFooter>
        </>
      )}
    </SettingsCard>
  );
}

export function ConfluenceSettings({ onStatusChange }: { onStatusChange?: StatusObserver } = {}) {
  return <SettingsStack><ConfluenceCard onStatusChange={onStatusChange} /></SettingsStack>;
}
