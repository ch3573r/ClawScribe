import { useState, useEffect } from 'react';
import { useRouter } from 'next/navigation';
import { invoke } from '@tauri-apps/api/core';
import { open } from '@tauri-apps/plugin-dialog';
import { Select, SelectContent, SelectGroup, SelectItem, SelectLabel, SelectTrigger, SelectValue } from './ui/select';
import { Input } from './ui/input';
import { Button } from './ui/button';
import { SettingsCard, SettingsFooter, SettingsRow } from '@/components/ui/settings-layout';
import { TranscriptionPcTest } from './TranscriptionPcTest';
import { TRANSCRIPTION_OPTIONS_ID } from '@/components/settings/transcriptionAnchors';
import { Eye, EyeOff, FlaskConical, Loader2, Lock, Unlock } from 'lucide-react';
import { ModelManager } from './WhisperModelManager';
import { ParakeetModelManager } from './ParakeetModelManager';
import { NemotronModelManager } from './NemotronModelManager';
import { WhisperAccelerationStatus } from './WhisperAccelerationStatus';
import { useCloudTranscription } from '@/hooks/useCloudTranscription';
import { toast } from 'sonner';
import { UnencryptedHttpOptIn } from './UnencryptedHttpOptIn';


export interface TranscriptModelProps {
    provider: 'localWhisper' | 'parakeet' | 'nemotron' | 'deepgram' | 'elevenLabs' | 'groq' | 'openai' | 'cloud-whisper' | 'mai-transcribe';
    model: string;
    apiKey?: string | null;
    baseUrl?: string | null;
    endpoint?: string | null;
    region?: string | null;
    allowUnencrypted?: boolean;
}

interface TranscriptProviderTestResult {
    provider: string;
    model: string;
    segmentCount: number;
    wordTimestampCount: number;
    requiresLocalTimingGrid: boolean;
    previewText: string;
}

export interface TranscriptSettingsProps {
    transcriptModelConfig: TranscriptModelProps;
    setTranscriptModelConfig: (config: TranscriptModelProps) => void;
    onModelSelect?: () => void;
    /** Offer "Test this PC". The Transcription page turns it on; modals leave it off. */
    pcTest?: boolean;
}

export function TranscriptSettings({ transcriptModelConfig, setTranscriptModelConfig, onModelSelect, pcTest = false }: TranscriptSettingsProps) {
    const [apiKey, setApiKey] = useState<string | null>(transcriptModelConfig.apiKey || null);
    const [apiKeyEdited, setApiKeyEdited] = useState(false);
    const [allowUnencrypted, setAllowUnencrypted] = useState(transcriptModelConfig.allowUnencrypted ?? false);
    const [showApiKey, setShowApiKey] = useState<boolean>(false);
    const [isApiKeyLocked, setIsApiKeyLocked] = useState<boolean>(true);
    const [isLockButtonVibrating, setIsLockButtonVibrating] = useState<boolean>(false);
    const [uiProvider, setUiProvider] = useState<TranscriptModelProps['provider']>(transcriptModelConfig.provider);
    const [cloudModel, setCloudModel] = useState<string>(transcriptModelConfig.model || 'whisper-1');
    const [cloudWhisperBaseUrl, setCloudWhisperBaseUrl] = useState<string>(transcriptModelConfig.baseUrl || 'https://api.openai.com/v1');
    const [maiEndpoint, setMaiEndpoint] = useState<string>(transcriptModelConfig.endpoint || '');
    const [maiRegion, setMaiRegion] = useState<string>(transcriptModelConfig.region || '');
    const [isTestingProvider, setIsTestingProvider] = useState<boolean>(false);
    const router = useRouter();
    const { enabled: cloudTranscriptionEnabled, loaded: cloudTranscriptionLoaded } = useCloudTranscription();
    const isCloudProvider = uiProvider === 'cloud-whisper' || uiProvider === 'mai-transcribe';
    const localEngineNames = { localWhisper: 'Whisper', parakeet: 'Parakeet', nemotron: 'Nemotron' };
    const savedLocalProvider = transcriptModelConfig.provider as keyof typeof localEngineNames;
    const liveEngine = localEngineNames[savedLocalProvider];
    // The live backend maps saved cloud providers to this default Parakeet model.
    const liveEngineLabel = liveEngine
        ? `${liveEngine} (${transcriptModelConfig.model})`
        : 'Parakeet (parakeet-tdt-0.6b-v3-int8)';

    // Sync uiProvider when backend config changes (e.g., after model selection or initial load)
    useEffect(() => {
        setUiProvider(transcriptModelConfig.provider);
        setAllowUnencrypted(transcriptModelConfig.allowUnencrypted ?? false);
    }, [transcriptModelConfig.provider]);

    useEffect(() => {
        if (transcriptModelConfig.provider === 'localWhisper' || transcriptModelConfig.provider === 'parakeet' || transcriptModelConfig.provider === 'nemotron') {
            setApiKey(null);
        }
    }, [transcriptModelConfig.provider]);

    useEffect(() => {
        setCloudModel(transcriptModelConfig.model || (transcriptModelConfig.provider === 'mai-transcribe' ? 'mai-transcribe-1.5' : 'whisper-1'));
        setCloudWhisperBaseUrl(transcriptModelConfig.baseUrl || 'https://api.openai.com/v1');
        setMaiEndpoint(transcriptModelConfig.endpoint || '');
        setMaiRegion(transcriptModelConfig.region || '');
    }, [transcriptModelConfig]);

    const fetchApiKey = async (provider: string) => {
        try {

            const data = await invoke('api_get_transcript_api_key', { provider }) as string;

            setApiKey(data || '');
            setApiKeyEdited(false);
        } catch (err) {
            console.error('Error fetching API key:', err);
            setApiKey(null);
        }
    };
    const engineDescription = ({
        parakeet: 'Parakeet runs on this PC in real time',
        localWhisper: 'Whisper runs on this PC with the highest accuracy',
        nemotron: 'Nemotron runs on this PC and streams in many languages',
    } as Record<string, string>)[uiProvider];
    // Hosted engines transcribe whole files only, so the speed test measures the local model live recording falls back to.
    const savedUsesParakeet = transcriptModelConfig.provider === 'parakeet' || transcriptModelConfig.provider === 'cloud-whisper' || transcriptModelConfig.provider === 'mai-transcribe';
    const parakeetTestTarget = savedUsesParakeet
        ? { provider: transcriptModelConfig.provider as string, model: transcriptModelConfig.model }
        : { provider: 'parakeet', model: '' };

    // The Options card sits on this page. Elsewhere (a modal) open the settings page instead.
    const openOptions = () => {
        const target = typeof document !== 'undefined' ? document.getElementById(TRANSCRIPTION_OPTIONS_ID) : null;
        if (target) {
            target.scrollIntoView({ block: 'start' });
            target.focus({ preventScroll: true });
            return;
        }
        router.push('/settings?tab=transcription');
        window.dispatchEvent(new CustomEvent('open-settings-tab', { detail: 'transcription' }));
    };

    const modelOptions = {
        localWhisper: [], // Model selection handled by ModelManager component
        parakeet: [], // Model selection handled by ParakeetModelManager component
        nemotron: [], // Model selection handled by NemotronModelManager component
        deepgram: ['nova-2-phonecall'],
        elevenLabs: ['eleven_multilingual_v2'],
        groq: ['llama-3.3-70b-versatile'],
        openai: ['gpt-4o'],
        'cloud-whisper': ['whisper-1'],
        'mai-transcribe': ['mai-transcribe-1.5'],
    };
    const requiresApiKey = uiProvider === 'deepgram' || uiProvider === 'elevenLabs' || uiProvider === 'openai' || uiProvider === 'groq' || isCloudProvider;

    const handleInputClick = () => {
        if (isApiKeyLocked) {
            setIsLockButtonVibrating(true);
            setTimeout(() => setIsLockButtonVibrating(false), 500);
        }
    };

    const handleWhisperModelSelect = (modelName: string) => {
        // Always update config when model is selected, regardless of current provider
        // This ensures the model is set when user switches back
        setTranscriptModelConfig({
            ...transcriptModelConfig,
            provider: 'localWhisper', // Ensure provider is set correctly
            model: modelName
        });
        // Close modal after selection
        if (onModelSelect) {
            onModelSelect();
        }
    };

    const handleParakeetModelSelect = (modelName: string) => {
        // Always update config when model is selected, regardless of current provider
        // This ensures the model is set when user switches back
        setTranscriptModelConfig({
            ...transcriptModelConfig,
            provider: 'parakeet', // Ensure provider is set correctly
            model: modelName
        });
        // Close modal after selection
        if (onModelSelect) {
            onModelSelect();
        }
    };

    const handleNemotronModelSelect = (modelName: string) => {
        setTranscriptModelConfig({
            ...transcriptModelConfig,
            provider: 'nemotron',
            model: modelName
        });
        if (onModelSelect) {
            onModelSelect();
        }
    };

    const getCloudProviderConfig = (): TranscriptModelProps => {
        const model = uiProvider === 'mai-transcribe'
            ? (cloudModel.trim() || 'mai-transcribe-1.5')
            : (cloudModel.trim() || 'whisper-1');
        return {
            ...transcriptModelConfig,
            provider: uiProvider,
            model,
            apiKey: apiKey || null,
            allowUnencrypted,
            baseUrl: uiProvider === 'cloud-whisper' ? cloudWhisperBaseUrl.trim() || 'https://api.openai.com/v1' : null,
            endpoint: uiProvider === 'mai-transcribe' ? maiEndpoint.trim() || null : null,
            region: uiProvider === 'mai-transcribe' ? maiRegion.trim() || null : null,
        };
    };

    const handleSaveCloudProvider = async () => {
        const nextConfig = getCloudProviderConfig();

        try {
            await invoke('api_save_transcript_config', {
                provider: nextConfig.provider,
                model: nextConfig.model,
                apiKey: nextConfig.apiKey,
                allowUnencrypted: nextConfig.allowUnencrypted,
                baseUrl: nextConfig.baseUrl,
                endpoint: nextConfig.endpoint,
                region: nextConfig.region,
            });
            setTranscriptModelConfig(nextConfig);
            setApiKeyEdited(false);
            toast.success('Transcription settings saved');
            if (onModelSelect) {
                onModelSelect();
            }
        } catch (err) {
            console.error('Failed to save cloud transcription settings:', err);
            toast.error(`Failed to save transcription settings: ${String(err)}`);
        }
    };

    const handleTestCloudProvider = async () => {
        const nextConfig = getCloudProviderConfig();
        if (!nextConfig.apiKey) {
            toast.error('Enter an API key before testing');
            return;
        }
        if (nextConfig.provider === 'mai-transcribe' && !nextConfig.endpoint) {
            toast.error('Enter an Azure Speech endpoint before testing');
            return;
        }

        setIsTestingProvider(true);
        try {
            const selected = await open({
                multiple: false,
                filters: [
                    {
                        name: 'Audio',
                        extensions: ['aac', 'flac', 'm4a', 'mp3', 'mp4', 'ogg', 'opus', 'wav', 'webm', 'wma'],
                    },
                ],
            });
            const audioPath = Array.isArray(selected) ? selected[0] : selected;
            if (!audioPath) {
                return;
            }

            const result = await invoke<TranscriptProviderTestResult>('api_test_transcript_provider', {
                provider: nextConfig.provider,
                model: nextConfig.model,
                apiKey: apiKeyEdited ? nextConfig.apiKey : null,
                allowUnencrypted: nextConfig.allowUnencrypted,
                baseUrl: nextConfig.baseUrl,
                endpoint: nextConfig.endpoint,
                region: nextConfig.region,
                audioPath,
                language: null,
            });
            const timingNote = result.requiresLocalTimingGrid
                ? ' MAI returned collapsed output; local timing-grid remap is required.'
                : '';
            toast.success(
                `Provider test passed: ${result.segmentCount} segment${result.segmentCount === 1 ? '' : 's'}, ${result.wordTimestampCount} word timestamp${result.wordTimestampCount === 1 ? '' : 's'}.${timingNote}`,
                { duration: 7000 }
            );
        } catch (err) {
            console.error('Hosted transcription provider test failed:', err);
            toast.error(`Provider test failed: ${String(err)}`, { duration: 9000 });
        } finally {
            setIsTestingProvider(false);
        }
    };


    const isLocalProvider = uiProvider === 'localWhisper' || uiProvider === 'parakeet' || uiProvider === 'nemotron';
    return (
        <div className="grid gap-5">
            <SettingsCard>
                <SettingsRow
                    title="Engine"
                    htmlFor="transcription-engine"
                    description={isCloudProvider
                        ? `Used for Import and Enhance (whole-file). Live recordings always transcribe on this device with ${liveEngineLabel}`
                        : engineDescription}
                >
                    <Select
                        value={uiProvider}
                        onValueChange={(value) => {
                            const provider = value as TranscriptModelProps['provider'];
                            setUiProvider(provider);
                            setAllowUnencrypted(false);
                            if (provider !== 'localWhisper' && provider !== 'parakeet' && provider !== 'nemotron') {
                                fetchApiKey(provider);
                            }
                            if (provider === 'cloud-whisper') {
                                setCloudModel(transcriptModelConfig.provider === 'cloud-whisper' ? transcriptModelConfig.model : 'whisper-1');
                                setCloudWhisperBaseUrl(transcriptModelConfig.baseUrl || 'https://api.openai.com/v1');
                            }
                            if (provider === 'mai-transcribe') {
                                setCloudModel('mai-transcribe-1.5');
                                setMaiEndpoint(transcriptModelConfig.endpoint || '');
                                setMaiRegion(transcriptModelConfig.region || '');
                            }
                        }}
                    >
                        <SelectTrigger id="transcription-engine" className="w-80 max-w-full">
                            <SelectValue placeholder="Select provider" />
                        </SelectTrigger>
                        <SelectContent>
                            <SelectGroup>
                                <SelectLabel className="normal-case">On your device</SelectLabel>
                                <SelectItem value="parakeet">Parakeet &middot; real-time, recommended</SelectItem>
                                <SelectItem value="localWhisper">Whisper &middot; highest accuracy</SelectItem>
                                <SelectItem value="nemotron">Nemotron &middot; streaming, multilingual (beta)</SelectItem>
                            </SelectGroup>
                            {(cloudTranscriptionEnabled || isCloudProvider) && (
                                <SelectGroup>
                                    <SelectLabel className="normal-case">Cloud APIs</SelectLabel>
                                    <SelectItem value="cloud-whisper" disabled={!cloudTranscriptionEnabled}>Hosted Whisper &middot; OpenAI-compatible</SelectItem>
                                    <SelectItem value="mai-transcribe" disabled={!cloudTranscriptionEnabled}>MAI-Transcribe &middot; Azure Speech</SelectItem>
                                </SelectGroup>
                            )}
                        </SelectContent>
                    </Select>

                    {!isLocalProvider && !isCloudProvider && (
                        <Select
                            value={transcriptModelConfig.model}
                            onValueChange={(value) => {
                                const model = value as TranscriptModelProps['model'];
                                setTranscriptModelConfig({ ...transcriptModelConfig, provider: uiProvider, model });
                            }}
                        >
                            <SelectTrigger aria-label="Model" className="w-56 max-w-full">
                                <SelectValue placeholder="Select model" />
                            </SelectTrigger>
                            <SelectContent>
                                {modelOptions[uiProvider].map((model) => (
                                    <SelectItem key={model} value={model}>{model}</SelectItem>
                                ))}
                            </SelectContent>
                        </Select>
                    )}
                </SettingsRow>
                {isCloudProvider && cloudTranscriptionLoaded && !cloudTranscriptionEnabled && (
                    <SettingsRow
                        role="status"
                        title="Cloud transcription is off"
                        description={`Import and Enhance use this device's engine. Your ${uiProvider === 'mai-transcribe' ? 'Microsoft AI' : 'Cloud Whisper'} settings are kept`}
                    >
                        <Button variant="outline" size="sm" onClick={openOptions}>Open options</Button>
                    </SettingsRow>
                )}
            </SettingsCard>

            {uiProvider === 'localWhisper' && (
                <div className="grid gap-5">
                    <WhisperAccelerationStatus />
                    <ModelManager
                        selectedModel={transcriptModelConfig.provider === 'localWhisper' ? transcriptModelConfig.model : undefined}
                        onModelSelect={handleWhisperModelSelect}
                        autoSave={true}
                    />
                </div>
            )}

            {uiProvider === 'parakeet' && (
                <ParakeetModelManager
                    selectedModel={transcriptModelConfig.provider === 'parakeet' ? transcriptModelConfig.model : undefined}
                    onModelSelect={handleParakeetModelSelect}
                    autoSave={true}
                    pcTest={pcTest ? parakeetTestTarget : undefined}
                />
            )}

            {uiProvider === 'nemotron' && (
                <NemotronModelManager
                    selectedModel={transcriptModelConfig.provider === 'nemotron' ? transcriptModelConfig.model : undefined}
                    onModelSelect={handleNemotronModelSelect}
                    autoSave={true}
                />
            )}

            {(isCloudProvider || requiresApiKey) && (
                <SettingsCard title="Connection">
                    {uiProvider === 'cloud-whisper' && cloudTranscriptionEnabled && (
                        <>
                            <SettingsRow title="Base URL" htmlFor="cloud-whisper-base-url">
                                <Input
                                    id="cloud-whisper-base-url"
                                    className="w-80 max-w-full"
                                    value={cloudWhisperBaseUrl}
                                    onChange={(e) => setCloudWhisperBaseUrl(e.target.value)}
                                    placeholder="https://api.openai.com/v1"
                                />
                            </SettingsRow>
                            <SettingsRow title="Model" htmlFor="cloud-whisper-model" description="OpenAI file uploads are limited to 25 MB. Larger recordings fall back to local transcription">
                                <Input
                                    id="cloud-whisper-model"
                                    className="w-80 max-w-full"
                                    value={cloudModel}
                                    onChange={(e) => setCloudModel(e.target.value)}
                                    placeholder="whisper-1"
                                />
                            </SettingsRow>
                        </>
                    )}

                    {uiProvider === 'mai-transcribe' && cloudTranscriptionEnabled && (
                        <>
                            <SettingsRow title="Azure Speech endpoint" htmlFor="mai-endpoint">
                                <Input
                                    id="mai-endpoint"
                                    className="w-80 max-w-full"
                                    value={maiEndpoint}
                                    onChange={(e) => setMaiEndpoint(e.target.value)}
                                    placeholder="https://your-resource.cognitiveservices.azure.com"
                                />
                            </SettingsRow>
                            <SettingsRow title="Region" htmlFor="mai-region">
                                <Input
                                    id="mai-region"
                                    className="w-80 max-w-full"
                                    value={maiRegion}
                                    onChange={(e) => setMaiRegion(e.target.value)}
                                    placeholder="eastus"
                                />
                            </SettingsRow>
                            <SettingsRow title="Model" htmlFor="mai-model" description="Sentence-level timing only, so speaker splitting is less precise">
                                <Input
                                    id="mai-model"
                                    className="w-80 max-w-full"
                                    value={cloudModel}
                                    onChange={(e) => setCloudModel(e.target.value)}
                                    placeholder="mai-transcribe-1.5"
                                />
                            </SettingsRow>
                        </>
                    )}

                    {isCloudProvider && (
                        <div className="px-5 py-3">
                            <UnencryptedHttpOptIn
                                urls={[uiProvider === 'cloud-whisper' ? cloudWhisperBaseUrl : maiEndpoint]}
                                checked={allowUnencrypted} onChange={setAllowUnencrypted}
                            />
                        </div>
                    )}

                    {requiresApiKey && (
                        <SettingsRow title="API key" htmlFor="transcription-api-key">
                            <div className="relative w-80 max-w-full">
                                <Input
                                    id="transcription-api-key"
                                    type={showApiKey ? "text" : "password"}
                                    className={`pr-20 ${isApiKeyLocked ? 'bg-muted cursor-not-allowed' : ''}`}
                                    value={apiKey || ''}
                                    onChange={(e) => { setApiKey(e.target.value); setApiKeyEdited(true); }}
                                    disabled={isApiKeyLocked}
                                    onClick={handleInputClick}
                                    placeholder="Enter your API key"
                                />
                                {isApiKeyLocked && (
                                    <div
                                        onClick={handleInputClick}
                                        className="absolute inset-0 flex items-center justify-center rounded-md cursor-not-allowed"
                                    />
                                )}
                                <div className="absolute inset-y-0 right-0 flex items-center pr-1">
                                    <Button
                                        type="button"
                                        variant="ghost"
                                        size="icon"
                                        onClick={() => setIsApiKeyLocked(!isApiKeyLocked)}
                                        className={`h-7 w-7 transition-colors duration-200 ${isLockButtonVibrating ? 'animate-vibrate text-destructive' : ''}`}
                                        aria-label={isApiKeyLocked ? "Unlock to edit the API key" : "Lock the API key"}
                                        title={isApiKeyLocked ? "Unlock to edit" : "Lock to prevent editing"}
                                    >
                                        {isApiKeyLocked ? <Lock className="h-4 w-4" /> : <Unlock className="h-4 w-4" />}
                                    </Button>
                                    <Button
                                        type="button"
                                        variant="ghost"
                                        size="icon"
                                        className="h-7 w-7"
                                        onClick={() => setShowApiKey(!showApiKey)}
                                        aria-label={showApiKey ? "Hide the API key" : "Show the API key"}
                                    >
                                        {showApiKey ? <EyeOff className="h-4 w-4" /> : <Eye className="h-4 w-4" />}
                                    </Button>
                                </div>
                            </div>
                        </SettingsRow>
                    )}

                    {isCloudProvider && cloudTranscriptionEnabled && (
                        <SettingsFooter
                            actions={<>
                                <Button
                                    type="button"
                                    variant="outline"
                                    size="sm"
                                    onClick={handleTestCloudProvider}
                                    disabled={isTestingProvider}
                                >
                                    {isTestingProvider ? (
                                        <Loader2 className="h-4 w-4 animate-spin" />
                                    ) : (
                                        <FlaskConical className="h-4 w-4" />
                                    )}
                                    Test
                                </Button>
                                <Button type="button" size="sm" onClick={handleSaveCloudProvider} disabled={isTestingProvider}>
                                    Save
                                </Button>
                            </>}
                        >
                            {isTestingProvider ? 'Testing with the audio file you chose' : 'Test uses an audio file you choose'}
                        </SettingsFooter>
                    )}
                </SettingsCard>
            )}

            {pcTest && uiProvider !== 'parakeet' && (
                <TranscriptionPcTest provider={transcriptModelConfig.provider} model={transcriptModelConfig.model} />
            )}
        </div>
    )
}
