import React, { useState, useEffect } from "react";
import { getVersion } from '@tauri-apps/api/app';
import Image from 'next/image';
import { UpdateDialog } from "./UpdateDialog";
import { updateService, UpdateInfo } from '@/services/updateService';
import { Button } from './ui/button';
import { Loader2, CheckCircle2 } from 'lucide-react';
import { toast } from 'sonner';


export function About() {
    const displayVersion = process.env.NEXT_PUBLIC_APP_VERSION ?? '';
    const [currentVersion, setCurrentVersion] = useState<string>(displayVersion);
    const [updateInfo, setUpdateInfo] = useState<UpdateInfo | null>(null);
    const [isChecking, setIsChecking] = useState(false);
    const [showUpdateDialog, setShowUpdateDialog] = useState(false);

    useEffect(() => {
        if (!displayVersion) {
            getVersion().then(setCurrentVersion).catch(console.error);
        }
    }, [displayVersion]);

    const handleCheckForUpdates = async () => {
        setIsChecking(true);
        try {
            const info = await updateService.checkForUpdates(true);
            setUpdateInfo(info);
            if (info.available) {
                setShowUpdateDialog(true);
            } else {
                toast.success('You are running the latest version');
            }
        } catch (error: any) {
            console.error('Failed to check for updates:', error);
            toast.error('Failed to check for updates: ' + (error.message || 'Unknown error'));
        } finally {
            setIsChecking(false);
        }
    };

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
                <div className="mt-3">
                    <Button
                        onClick={handleCheckForUpdates}
                        disabled={isChecking}
                        variant="outline"
                        size="sm"
                        className="text-xs"
                    >
                        {isChecking ? (
                            <>
                                <Loader2 className="h-3 w-3 mr-2 animate-spin" />
                                Checking...
                            </>
                        ) : (
                            <>
                                <CheckCircle2 className="h-3 w-3 mr-2" />
                                Check for Updates
                            </>
                        )}
                    </Button>
                    {updateInfo?.available && (
                        <div className="mt-2 text-xs text-primary">
                            Update available: v{updateInfo.version}
                        </div>
                    )}
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

            {/* Coming Soon - Compact */}
            <div className="bg-primary/10 rounded p-3">
                <p className="text-s text-primary">
                    <span className="font-bold">Coming soon:</span> Meeting workflows for follow-ups, action tracking, and structured handoff.
                </p>
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
                    Advanced Codex app-server bundles @openai/codex 0.139.0 for Windows x64 under Apache-2.0. Runtime SHA256: 77a84f8078400467ade4301d827b8bcea2d29b6838c9cd162bf3573b7ef97e10.
                </p>
            </div>

            {/* Update Dialog */}
            <UpdateDialog
                open={showUpdateDialog}
                onOpenChange={setShowUpdateDialog}
                updateInfo={updateInfo}
            />
        </div>

    )
}
