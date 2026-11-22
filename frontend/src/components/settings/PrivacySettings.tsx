"use client";

import { Loader2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Meta, SettingsCard, SettingsColumns, SettingsFooter, StatusDot, StatusText, type StatusTone } from "@/components/ui/settings-layout";
import { usePrivacyInput } from "@/components/settings/usePrivacyInput";
import { derivePrivacy, type PrivacyFact, type PrivacyRow, type PrivacyTone } from "@/lib/privacy";

const toStatusTone = (tone: PrivacyTone): StatusTone => tone;

export function openSettingsTab(tab: string) {
  window.dispatchEvent(new CustomEvent("open-settings-tab", { detail: tab }));
}

function Fact({ fact }: { fact: PrivacyFact }) {
  return (
    <div className="min-w-0 px-5 py-4">
      <div className="text-xs text-muted-foreground">{fact.label}</div>
      <div className="mt-2 flex items-center gap-2.5 text-lg font-semibold text-foreground">
        {fact.state === "loading" ? <Loader2 aria-hidden="true" className="h-4 w-4 animate-spin text-muted-foreground" /> : <StatusDot tone={toStatusTone(fact.tone)} className="h-2.5 w-2.5" />}
        <span className={fact.tone === "warning" ? "text-warning-foreground" : undefined}>{fact.value}</span>
      </div>
      {fact.detail && <div className="mt-1 text-[13px] leading-5 text-muted-foreground">{fact.detail}</div>}
    </div>
  );
}

function DataRow({ row }: { row: PrivacyRow }) {
  return (
    <tr className="align-top">
      <td className="px-5 py-3.5">
        <div className="text-foreground">{row.title}</div>
        {row.detail && (
          row.detailIsPath
            ? <Meta className="mt-0.5 block whitespace-normal break-all" ><span title={row.detail}>{row.detail}</span></Meta>
            : <div className="mt-0.5 text-[13px] leading-5 text-muted-foreground">{row.detail}</div>
        )}
      </td>
      <td className="px-3 py-3.5 text-foreground">
        {row.state === "loading"
          ? <span role="status" className="inline-flex items-center gap-2 text-muted-foreground"><Loader2 aria-hidden="true" className="h-3.5 w-3.5 animate-spin" />Checking…</span>
          : row.state === "unavailable"
            ? <span className="text-muted-foreground">{row.destination}</span>
            : row.destination}
      </td>
      <td className="px-3 py-3.5">
        {row.state === "ready" && <StatusText tone={toStatusTone(row.tone)} className="whitespace-normal">{row.when}</StatusText>}
      </td>
      <td className="px-5 py-2.5 text-right">
        <Button type="button" variant="link" size="sm" className="h-auto p-0" aria-label={`Open ${row.tabLabel} settings`} onClick={() => openSettingsTab(row.tab)}>
          {row.tabLabel}
        </Button>
      </td>
    </tr>
  );
}

/** What stays on this PC and what leaves it, derived from the settings that control it. */
export function PrivacySettings() {
  const model = derivePrivacy(usePrivacyInput());

  return (
    <SettingsColumns
      aside={
        <>
          <SettingsCard title="How to read this">
            <div className="grid gap-4 p-5">
              {([
                ["neutral", "Never leaves", "Processed and stored on this PC only"],
                ["active", "On your action", "Sent only when you press generate, ask or export"],
                ["warning", "Automatic", "Happens without you starting it"],
              ] as const).map(([tone, title, text]) => (
                <div key={title} className="grid grid-cols-[0.5rem_1fr] items-baseline gap-x-3 gap-y-0.5">
                  <StatusDot tone={tone} />
                  <span className="text-sm text-foreground">{title}</span>
                  <span />
                  <span className="text-[13px] leading-5 text-muted-foreground">{text}</span>
                </div>
              ))}
            </div>
          </SettingsCard>
          <SettingsCard
            title="Keep everything local"
            description="Use a built-in or Ollama summary model. Keep cloud transcription, live assistance, OpenClaw handoff and webhooks off."
          >
            <SettingsFooter>
              <Button type="button" variant="outline" size="sm" onClick={() => openSettingsTab("summary")}>Open summary</Button>
            </SettingsFooter>
          </SettingsCard>
        </>
      }
    >
      <SettingsCard aria-label="Summary of what leaves this PC">
        <div className="grid divide-y divide-border/60 min-[700px]:grid-cols-3 min-[700px]:divide-x min-[700px]:divide-y-0">
          {model.facts.map((fact) => <Fact key={fact.id} fact={fact} />)}
        </div>
      </SettingsCard>

      <SettingsCard aria-label="Where each kind of data goes">
        <table className="w-full text-sm">
          <thead>
            <tr className="border-b border-border/60 text-left text-xs text-muted-foreground">
              <th scope="col" className="px-5 py-3 font-normal">Data</th>
              <th scope="col" className="px-3 py-3 font-normal">Where it goes</th>
              <th scope="col" className="px-3 py-3 font-normal">When</th>
              <th scope="col" className="px-5 py-3"><span className="sr-only">Setting</span></th>
            </tr>
          </thead>
          <tbody className="divide-y divide-border/60">
            {model.rows.map((row) => <DataRow key={row.id} row={row} />)}
          </tbody>
        </table>
      </SettingsCard>
    </SettingsColumns>
  );
}
