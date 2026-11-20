import { useState, useEffect } from 'react';
import { useRouter } from 'next/navigation';
import { invoke } from '@tauri-apps/api/core';
import { Select, SelectContent, SelectGroup, SelectItem, SelectLabel, SelectTrigger, SelectValue } from './ui/select';
import { Input } from './ui/input';
import { Button } from './ui/button';
import { SettingsCard, SettingsFooter, SettingsRow } from '@/components/ui/settings-layout';
import { TranscriptionPcTest } from './TranscriptionPcTest';
import { TRANSCRIPTION_OPTIONS_ID } from '@/components/settings/transcriptionAnchors';
import { FlaskConical, Loader2 } from 'lucide-react';
import { ModelManager } from './WhisperModelManager';
import { ParakeetModelManager } from './ParakeetModelManager';
import { NemotronModelManager } from './NemotronModelManager';
import { WhisperAccelerationStatus } from './WhisperAccelerationStatus';
import { useCloudTranscription } from '@/hooks/useCloudTranscription';
import { toast } from 'sonner';
import { UnencryptedHttpOptIn } from './UnencryptedHttpOptIn';
import { SecretKeyRow } from '@/components/settings/SecretKeyControl';
import { configService } from '@/services/configService';
import { KEY_REQUIRED_FOR_NEW_ADDRESS, NO_SECRET, hasUsableKey, keyProblem, keyRequiredForAddress, newKeyPayload, removalNeedsConfirmation, type SecretStatus } from '@/lib/secret-key';


export interface TranscriptModelProps {
    provider: 'localWhisper' | 'parakeet' | 'nemotron' | 'deepgram' | 'elevenLabs' | 'groq' | 'openai' | 'cloud-whisper' | 'mai-transcribe';
    model: string;
    /** A key the user has just typed, held only until it is saved. Never loaded from the core. */
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
    // The key typed for the provider being looked at. A stored key is never loaded
    // here: the core reports only whether one is stored.
    const [apiKey, setApiKey] = useState<string | null>(null);
    const [keyStatus, setKeyStatus] = useState<SecretStatus>(NO_SECRET);
    const [allowUnencrypted, setAllowUnencrypted] = useState(transcriptModelConfig.allowUnencrypted ?? false);
    const [uiProvider, setUiProvider] = useState<TranscriptModelProps['provider']>(transcriptModelConfig.provider);
    const [liveModelRepair, setLiveModelRepair] = useState(false);
    const [cloudModel, setCloudModel] = useState<string>(transcriptModelConfig.model || 'whisper-1');
    const [cloudWhisperBaseUrl, setCloudWhisperBaseUrl] = useState<string>(transcriptModelConfig.baseUrl || 'https://api.openai.com/v1');
    const [maiEndpoint, setMaiEndpoint] = useState<string>(transcriptModelConfig.endpoint || '');
    const [maiRegion, setMaiRegion] = useState<string>(transcriptModelConfig.region || '');
    const [isTestingProvider, setIsTestingProvider] = useState<boolean>(false);
    const router = useRouter();
    const { enabled: cloudTranscriptionEnabled, loaded: cloudTranscriptionLoaded } = useCloudTranscription();
    const isCloudProvider = uiProvider === 'cloud-whisper' || uiProvider === 'mai-transcribe';
    const hostedProviderSaved = transcriptModelConfig.provider === 'cloud-whisper' || transcriptModelConfig.provider === 'mai-transcribe';
    const parakeetDownloadOnly = liveModelRepair && hostedProviderSaved && uiProvider === 'parakeet';
    const localEngineNames = { localWhisper: 'Whisper', parakeet: 'Parakeet', nemotron: 'Nemotron' };
    const savedLocalProvider = transcriptModelConfig.provider as keyof typeof localEngineNames;
    const liveEngine = localEngineNames[savedLocalProvider];
    // The live backend maps saved cloud providers to this default Parakeet model.
    const liveEngineLabel = liveEngine
        ? `${liveEngine} (${transcriptModelConfig.model})`
        : 'Parakeet (parakeet-tdt-0.6b-v3-int8)';

    // Sync uiProvider when backend config changes (e.g., after model selection or initial load)
    useEffect(() => {
        const engine = pcTest && typeof window !== 'undefined'
            ? new URLSearchParams(window.location?.search).get('engine')
            : null;
        setLiveModelRepair(engine === 'parakeet');
        setUiProvider(engine === 'parakeet' || engine === 'localWhisper' || engine === 'nemotron'
            ? engine
            : transcriptModelConfig.provider);
        setAllowUnencrypted(transcriptModelConfig.allowUnencrypted ?? false);
    }, [pcTest, transcriptModelConfig.provider]);

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

    // Which hosted provider's key is stored. The key itself stays in the core.
    const loadKeyStatus = async (provider: string) => {
        setApiKey(null);
        try {
            setKeyStatus(await configService.getTranscriptApiKeyStatus(provider));
        } catch {
            console.error('Error reading the API key status');
            setKeyStatus(NO_SECRET);
        }
    };

    useEffect(() => {
        if (transcriptModelConfig.provider === 'cloud-whisper' || transcriptModelConfig.provider === 'mai-transcribe') {
            void loadKeyStatus(transcriptModelConfig.provider);
        }
    }, [transcriptModelConfig.provider]);

    const removeStoredKey = async () => {
        await configService.clearTranscriptApiKey(uiProvider);
        setApiKey(null);
        await loadKeyStatus(uiProvider);
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
            // Only a key typed now is sent; without one the stored key is kept.
            apiKey: newKeyPayload(apiKey),
            allowUnencrypted,
            baseUrl: uiProvider === 'cloud-whisper' ? cloudWhisperBaseUrl.trim() || 'https://api.openai.com/v1' : null,
            endpoint: uiProvider === 'mai-transcribe' ? maiEndpoint.trim() || null : null,
            region: uiProvider === 'mai-transcribe' ? maiRegion.trim() || null : null,
        };
    };

    // A stored key goes only to the address it was saved for: changing the
    // address asks for the key again, and nothing is saved until it is entered.
    const savedCloudAddress = uiProvider === 'cloud-whisper'
        ? (transcriptModelConfig.baseUrl || 'https://api.openai.com/v1')
        : (transcriptModelConfig.endpoint || '');
    const editedCloudAddress = uiProvider === 'cloud-whisper'
        ? (cloudWhisperBaseUrl.trim() || 'https://api.openai.com/v1')
        : maiEndpoint.trim();
    const cloudKeyRequired = (uiProvider === 'cloud-whisper' || uiProvider === 'mai-transcribe')
        && !!savedCloudAddress.trim()
        && keyRequiredForAddress(savedCloudAddress, editedCloudAddress, keyStatus, apiKey);

    const handleSaveCloudProvider = async () => {
        const nextConfig = getCloudProviderConfig();
        const problem = keyProblem(apiKey);
        if (problem) { toast.error(problem); return; }
        if (cloudKeyRequired) { toast.error(KEY_REQUIRED_FOR_NEW_ADDRESS); return; }

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
            // The typed key is stored now: keep and share only the rest.
            setTranscriptModelConfig({ ...nextConfig, apiKey: null });
            await loadKeyStatus(nextConfig.provider);
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
        const problem = keyProblem(apiKey);
        if (problem) { toast.error(problem); return; }
        if (!hasUsableKey(nextConfig.apiKey, keyStatus)) {
            toast.error('Enter an API key before testing');
            return;
        }
        if (nextConfig.provider === 'mai-transcribe' && !nextConfig.endpoint) {
            toast.error('Enter an Azure Speech endpoint before testing');
            return;
        }

        setIsTestingProvider(true);
        try {
            // The core opens the dialog and only accepts the file it chose here.
            const audioPath = await invoke<string | null>('pick_transcription_test_audio');
            if (!audioPath) {
                return;
            }

            const result = await invoke<TranscriptProviderTestResult>('api_test_transcript_provider', {
                provider: nextConfig.provider,
                model: nextConfig.model,
                // A typed key, or none: the stored key is used inside the core, for its own endpoint.
                apiKey: nextConfig.apiKey,
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
                    description={parakeetDownloadOnly
                        ? 'Parakeet is used for live recordings. Downloading a model here keeps your hosted file-transcription preference'
                        : isCloudProvider
                        ? `Used for Import and Enhance (whole-file). Live recordings always transcribe on this device with ${liveEngineLabel}`
                        : engineDescription}
                >
                    <Select
                        value={uiProvider}
                        onValueChange={(value) => {
                            const provider = value as TranscriptModelProps['provider'];
                            setLiveModelRepair(false);
                            setUiProvider(provider);
                            setAllowUnencrypted(false);
                            if (provider !== 'localWhisper' && provider !== 'parakeet' && provider !== 'nemotron') {
                                void loadKeyStatus(provider);
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
                    onModelSelect={parakeetDownloadOnly ? undefined : handleParakeetModelSelect}
                    autoSave={!parakeetDownloadOnly}
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
                        <SecretKeyRow
                            id="transcription-api-key" title="API key" status={keyStatus}
                            draft={apiKey || ''} onDraftChange={(value) => setApiKey(value || null)}
                            required={cloudKeyRequired ? KEY_REQUIRED_FOR_NEW_ADDRESS : null}
                            onRemove={removeStoredKey}
                            removeWarning={removalNeedsConfirmation(uiProvider, transcriptModelConfig.provider)
                                ? 'Hosted transcription with this provider will not work until you add a key again.'
                                : null}
                            description="Protected by your operating system. Saved when you choose Save"
                        />
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
