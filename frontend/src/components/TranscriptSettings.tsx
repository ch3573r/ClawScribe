import { useState, useEffect, useRef } from 'react';
import { useRouter } from 'next/navigation';
import { invoke } from '@tauri-apps/api/core';
import { Select, SelectContent, SelectGroup, SelectItem, SelectLabel, SelectTrigger, SelectValue } from './ui/select';
import { Input } from './ui/input';
import { Button } from './ui/button';
import { SettingsCard, SettingsFooter, SettingsRow } from '@/components/ui/settings-layout';
import { TranscriptionPcTest } from './TranscriptionPcTest';
import { TRANSCRIPTION_OPTIONS_ID } from '@/components/settings/transcriptionAnchors';
import { FlaskConical, Loader2 } from 'lucide-react';
import { getWhisperModelDisplayName } from '@/lib/whisper';
import { getModelDisplayName } from '@/lib/parakeet';
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
    const saving = useRef(false);
    const [savingModel, setSavingModel] = useState(false);
    const { provider, model } = transcriptModelConfig;
    const selectedLabel = {
        localWhisper: 'Whisper', parakeet: 'Parakeet', nemotron: 'Nemotron',
        'cloud-whisper': 'Hosted Whisper', 'mai-transcribe': 'Azure Speech',
        deepgram: 'Deepgram', elevenLabs: 'ElevenLabs', groq: 'Groq', openai: 'OpenAI',
    }[provider];
    const selectedModel = provider === 'parakeet' ? getModelDisplayName(model)
        : provider === 'localWhisper' ? getWhisperModelDisplayName(model)
        : provider === 'mai-transcribe' ? 'Standard transcription'
        : provider === 'nemotron' ? model.replace(/^nemotron-streaming-0\.6b-(int8|fp16)$/, (_, variant: string) => variant.toUpperCase()) : model;

    const browsedByUser = useRef(false);
    useEffect(() => {
        if (browsedByUser.current) return;
        const engine = pcTest && typeof window !== 'undefined'
            ? new URLSearchParams(window.location?.search).get('engine')
            : null;
        setLiveModelRepair(engine === 'parakeet');
        setUiProvider(engine === 'parakeet' || engine === 'localWhisper' || engine === 'nemotron'
            ? engine
            : transcriptModelConfig.provider);
    }, [pcTest, transcriptModelConfig.provider]);

    useEffect(() => {
        setAllowUnencrypted(transcriptModelConfig.allowUnencrypted ?? false);
    }, [transcriptModelConfig.provider, transcriptModelConfig.allowUnencrypted]);

    useEffect(() => {
        if (transcriptModelConfig.provider === 'localWhisper' || transcriptModelConfig.provider === 'parakeet' || transcriptModelConfig.provider === 'nemotron') {
            setApiKey(null);
        }
    }, [transcriptModelConfig.provider]);

    useEffect(() => {
        setCloudModel(transcriptModelConfig.provider === 'mai-transcribe' ? 'azure-speech' : transcriptModelConfig.model || 'whisper-1');
        setCloudWhisperBaseUrl(transcriptModelConfig.baseUrl || 'https://api.openai.com/v1');
        setMaiEndpoint(transcriptModelConfig.endpoint || '');
        setMaiRegion(transcriptModelConfig.region || '');
    }, [transcriptModelConfig]);

    // Which hosted provider's key is stored. The key itself stays in the core.
    const keyRequest = useRef(0);
    const loadKeyStatus = async (provider: string) => {
        const revision = ++keyRequest.current;
        setKeyStatus(NO_SECRET);
        setApiKey(null);
        try {
            const status = await configService.getTranscriptApiKeyStatus(provider);
            if (revision === keyRequest.current) setKeyStatus(status);
        } catch {
            console.error('Error reading the API key status');
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
        'mai-transcribe': ['azure-speech'],
    };
    const requiresApiKey = uiProvider === 'deepgram' || uiProvider === 'elevenLabs' || uiProvider === 'openai' || uiProvider === 'groq' || isCloudProvider;

    const selectLocalModel = async (provider: 'localWhisper' | 'parakeet' | 'nemotron', model: string) => {
        if (saving.current) return;
        saving.current = true;
        setSavingModel(true);
        try {
            await invoke('api_save_transcript_config', { provider, model, apiKey: null });
            setTranscriptModelConfig({ ...transcriptModelConfig, provider, model, apiKey: null, allowUnencrypted: false });
            onModelSelect?.();
        } catch {
            toast.error('Could not save the speech model. Try again.');
        } finally {
            saving.current = false;
            setSavingModel(false);
        }
    };

    const getCloudProviderConfig = (): TranscriptModelProps => {
        const model = uiProvider === 'mai-transcribe'
            ? 'azure-speech'
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
        if (saving.current || !cloudTranscriptionEnabled) return;
        const nextConfig = getCloudProviderConfig();
        const problem = keyProblem(apiKey);
        if (problem) { toast.error(problem); return; }
        if (cloudKeyRequired) { toast.error(KEY_REQUIRED_FOR_NEW_ADDRESS); return; }

        saving.current = true;
        setSavingModel(true);
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
        } finally {
            saving.current = false;
            setSavingModel(false);
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
                ? ' The provider returned combined text; local timing is approximate.'
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
                    role="status"
                    title="Selected transcription model"
                    description={hostedProviderSaved
                        ? `${selectedLabel} · ${selectedModel} (${cloudTranscriptionLoaded && !cloudTranscriptionEnabled ? 'file transcription, off' : 'file transcription'}). Live transcription uses an installed Parakeet model.`
                        : `${selectedLabel} · ${selectedModel || 'Automatic model selection'}`}
                />
                <SettingsRow
                    title="Browse engines"
                    htmlFor="transcription-engine"
                    description={parakeetDownloadOnly
                        ? 'Parakeet is used for live recordings. Downloading a model here keeps your hosted file-transcription preference'
                        : isCloudProvider
                        ? 'Choose Save to use this provider for file transcription.'
                        : 'Choose Use beside an installed model to switch. Download a model first if needed.'}
                >
                    <Select
                        disabled={savingModel}
                        value={uiProvider}
                        onValueChange={(value) => {
                            const provider = value as TranscriptModelProps['provider'];
                            browsedByUser.current = true;
                            setLiveModelRepair(false);
                            setUiProvider(provider);
                            setAllowUnencrypted(provider === transcriptModelConfig.provider && (transcriptModelConfig.allowUnencrypted ?? false));
                            if (provider !== 'localWhisper' && provider !== 'parakeet' && provider !== 'nemotron') {
                                void loadKeyStatus(provider);
                            }
                            if (provider === 'cloud-whisper') {
                                setCloudModel(transcriptModelConfig.provider === 'cloud-whisper' ? transcriptModelConfig.model : 'whisper-1');
                                setCloudWhisperBaseUrl(transcriptModelConfig.baseUrl || 'https://api.openai.com/v1');
                            }
                            if (provider === 'mai-transcribe') {
                                setCloudModel('azure-speech');
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
                                <SelectItem value="parakeet">Parakeet</SelectItem>
                                <SelectItem value="localWhisper">Whisper</SelectItem>
                                <SelectItem value="nemotron">Nemotron &middot; streaming, multilingual (beta)</SelectItem>
                            </SelectGroup>
                            {(cloudTranscriptionEnabled || isCloudProvider) && (
                                <SelectGroup>
                                    <SelectLabel className="normal-case">Cloud APIs</SelectLabel>
                                    <SelectItem value="cloud-whisper" disabled={!cloudTranscriptionEnabled}>Hosted Whisper &middot; OpenAI-compatible</SelectItem>
                                    <SelectItem value="mai-transcribe" disabled={!cloudTranscriptionEnabled}>Azure Speech</SelectItem>
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
                        description={uiProvider === provider
                            ? 'Import and Enhance use this device. Your saved provider settings are kept.'
                            : 'Import and Enhance use this device. Enable cloud transcription in Options to configure a hosted provider.'}
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
                        onModelSelect={savingModel ? undefined : name => selectLocalModel('localWhisper', name)}
                    />
                </div>
            )}

            {uiProvider === 'parakeet' && (
                <ParakeetModelManager
                    selectedModel={transcriptModelConfig.provider === 'parakeet' ? transcriptModelConfig.model : undefined}
                    onModelSelect={parakeetDownloadOnly || savingModel ? undefined : name => selectLocalModel('parakeet', name)}
                    pcTest={pcTest ? parakeetTestTarget : undefined}
                />
            )}

            {uiProvider === 'nemotron' && (
                <NemotronModelManager
                    selectedModel={transcriptModelConfig.provider === 'nemotron' ? transcriptModelConfig.model : undefined}
                    onModelSelect={savingModel ? undefined : name => selectLocalModel('nemotron', name)}
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
                            <SettingsRow title="Azure Speech endpoint" htmlFor="mai-endpoint" description="Use the Speech resource endpoint, without a Foundry project path. Standard transcription selects its model automatically">
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
                                    disabled={savingModel || isTestingProvider}
                                >
                                    {isTestingProvider ? (
                                        <Loader2 className="h-4 w-4 animate-spin" />
                                    ) : (
                                        <FlaskConical className="h-4 w-4" />
                                    )}
                                    Test
                                </Button>
                                <Button type="button" size="sm" onClick={handleSaveCloudProvider} disabled={savingModel || isTestingProvider}>
                                    Save
                                </Button>
                            </>}
                        >
                            {isTestingProvider ? 'Testing with the audio file you chose' : 'Test uses an audio file you choose'}
                        </SettingsFooter>
                    )}
                </SettingsCard>
            )}

            {pcTest && uiProvider !== 'parakeet' && uiProvider === provider && (
                <TranscriptionPcTest provider={transcriptModelConfig.provider} model={transcriptModelConfig.model} />
            )}
        </div>
    )
}
