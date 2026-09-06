import React from "react";
import Image from 'next/image';
import { UpdateChannelSettings } from './UpdateChannelSettings';
import { useUpdateCheckContext } from './UpdateCheckProvider';

export function About() {
    const { currentVersion } = useUpdateCheckContext();

    return (
        <div className="p-4 space-y-4 h-[80vh] overflow-y-auto">
            {/* Compact Header */}
            <div className="text-center">
                <div className="mb-3">
                    <Image
                        src="icon_128x128.png"
                        alt="ClawScribe Logo"
                        width={64}
                        height={64}
                        className="mx-auto"
                    />
                </div>
                <h1 className="text-xl font-bold text-foreground">ClawScribe</h1>
                <span className="text-sm text-muted-foreground"> v{currentVersion}</span>
                <p className="text-xs text-muted-foreground mt-1">
                    Based on Meetily Community Edition 0.4.0
                </p>
                <p className="text-medium text-muted-foreground mt-1">
                    Local meeting capture, transcripts, summaries, and OpenClaw handoff.
                </p>
                <div className="mt-4 rounded-md border border-border bg-card p-4">
                    <UpdateChannelSettings id="about-prerelease-updates" />
                </div>
            </div>

            {/* Features Grid - Compact */}
            <div className="space-y-3">
                <h2 className="text-base font-semibold text-foreground">What makes ClawScribe different</h2>
                <div className="grid grid-cols-2 gap-2">
                    <div className="bg-muted rounded p-3 hover:bg-muted transition-colors">
                        <h3 className="font-bold text-sm text-foreground mb-1">Privacy-first</h3>
                        <p className="text-xs text-muted-foreground leading-relaxed">Your data & AI processing workflow can now stay within your premise. No cloud, no leaks.</p>
                    </div>
                    <div className="bg-muted rounded p-3 hover:bg-muted transition-colors">
                        <h3 className="font-bold text-sm text-foreground mb-1">Use Any Model</h3>
                        <p className="text-xs text-muted-foreground leading-relaxed">Prefer local open-source model? Great. Want to plug in an external API? Also fine. No lock-in.</p>
                    </div>
                    <div className="bg-muted rounded p-3 hover:bg-muted transition-colors">
                        <h3 className="font-bold text-sm text-foreground mb-1">Cost-Smart</h3>
                        <p className="text-xs text-muted-foreground leading-relaxed">Avoid pay-per-minute bills by running models locally (or pay only for the calls you choose).</p>
                    </div>
                    <div className="bg-muted rounded p-3 hover:bg-muted transition-colors">
                        <h3 className="font-bold text-sm text-foreground mb-1">Works everywhere</h3>
                        <p className="text-xs text-muted-foreground leading-relaxed">Google Meet, Zoom, Teams-online or offline.</p>
                    </div>
                </div>
            </div>

            {/* CTA Section - Compact */}
            <div className="text-center space-y-2">
                <h3 className="text-medium font-semibold text-foreground">Need a private meeting workflow?</h3>
                <p className="text-s text-muted-foreground">
                    ClawScribe is tuned for local-first capture and optional OpenClaw processing without a visible meeting bot.
                </p>
            </div>

            {/* Footer - Compact */}
            <div className="pt-2 border-t border-border text-center">
                <p className="text-xs text-muted-foreground">
                    ClawScribe is an OpenClaw fork of Meetily Community Edition. Meetily is copyright Zackriya Solutions and contributors under the MIT License.
                </p>
                <p className="text-xs text-muted-foreground mt-2">
                    Advanced Codex app-server bundles @openai/codex 0.144.1 for Windows x64 under Apache-2.0. Runtime SHA256: cbacbb9726262ef558b4af0438a1b2a5bba9076132401d947b5b4d2bf92ab0e4.
                </p>
            </div>

        </div>

    )
}
