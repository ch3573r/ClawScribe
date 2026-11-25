import Image from "next/image";
import { Button } from "@/components/ui/button";
import { SettingsCard, SettingsRow, SettingsStack } from "@/components/ui/settings-layout";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { UpdateChannelSettings } from "./UpdateChannelSettings";
import { useUpdateCheckContext } from "./UpdateCheckProvider";

export function About({ compact = false }: { compact?: boolean }) {
  const { currentVersion } = useUpdateCheckContext();
  return (
    <SettingsStack>
      <SettingsCard>
        <div className="flex items-center gap-4 px-5 py-4">
          <Image src="/brand/clawscribe-icon-64.png" alt="" width={48} height={48} className="rounded-md" />
          <div className="min-w-0">
            <p className="font-semibold">ClawScribe{currentVersion ? ` ${currentVersion}` : ""}</p>
            <p className="mt-1 text-sm text-muted-foreground">Local meeting recording, transcription and summaries.</p>
          </div>
        </div>
        <UpdateChannelSettings id={compact ? "about-dialog-prerelease" : "about-settings-prerelease"} />
      </SettingsCard>
      <SettingsCard title="Licensing and bundled software">
        <SettingsRow title="Meetily Community Edition" description="ClawScribe is an OpenClaw fork of Meetily Community Edition 0.4.0, copyright Zackriya Solutions and contributors under the MIT License." />
        <SettingsRow title="Codex app-server" description="The optional Codex provider bundles @openai/codex 0.169.0 for Windows x64 under Apache-2.0.">
          <Tooltip>
            <TooltipTrigger asChild><Button type="button" variant="outline" size="sm">Runtime checksum</Button></TooltipTrigger>
            <TooltipContent className="max-w-72 break-all">SHA-256 ce1945eb440b5481ed1233ebaf32b7056ee7e4e452fa62ffe1cbfb26103cdee0</TooltipContent>
          </Tooltip>
        </SettingsRow>
        <SettingsRow title="Claude Code" description="Uses your own Claude Code installation and sign-in. Your plan's limits apply." />
      </SettingsCard>
    </SettingsStack>
  );
}
