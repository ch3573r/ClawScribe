import { useState, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { open } from '@tauri-apps/plugin-dialog';
import { Select, SelectContent, SelectGroup, SelectItem, SelectLabel, SelectTrigger, SelectValue } from './ui/select';
import { Input } from './ui/input';
import { Button } from './ui/button';
import { Label } from './ui/label';
import { Eye, EyeOff, FlaskConical, Loader2, Lock, Unlock } from 'lucide-react';
import { ModelManager } from './WhisperModelManager';
import { ParakeetModelManager } from './ParakeetModelManager';
import { NemotronModelManager } from './NemotronModelManager';
import { WhisperAccelerationStatus } from './WhisperAccelerationStatus';
import { useCloudTranscription } from '@/hooks/useCloudTranscription';
import { toast } from 'sonner';


export interface TranscriptModelProps {
    provider: 'localWhisper' | 'parakeet' | 'nemotron' | 'deepgram' | 'elevenLabs' | 'groq' | 'openai' | 'cloud-whisper' | 'mai-transcribe';
    model: string;
    apiKey?: string | null;
    baseUrl?: string | null;
    endpoint?: string | null;
    region?: string | null;
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
}

export function TranscriptSettings({ transcriptModelConfig, setTranscriptModelConfig, onModelSelect }: TranscriptSettingsProps) {
    const [apiKey, setApiKey] = useState<string | null>(transcriptModelConfig.apiKey || null);
    const [showApiKey, setShowApiKey] = useState<boolean>(false);
    const [isApiKeyLocked, setIsApiKeyLocked] = useState<boolean>(true);
    const [isLockButtonVibrating, setIsLockButtonVibrating] = useState<boolean>(false);
    const [uiProvider, setUiProvider] = useState<TranscriptModelProps['provider']>(transcriptModelConfig.provider);
    const [cloudModel, setCloudModel] = useState<string>(transcriptModelConfig.model || 'whisper-1');
    const [cloudWhisperBaseUrl, setCloudWhisperBaseUrl] = useState<string>(transcriptModelConfig.baseUrl || 'https://api.openai.com/v1');
    const [maiEndpoint, setMaiEndpoint] = useState<string>(transcriptModelConfig.endpoint || '');
    const [maiRegion, setMaiRegion] = useState<string>(transcriptModelConfig.region || '');
    const [isTestingProvider, setIsTestingProvider] = useState<boolean>(false);
    const cloudTranscriptionEnabled = useCloudTranscription();
    const isCloudProvider = uiProvider === 'cloud-whisper' || uiProvider === 'mai-transcribe';

    // Sync uiProvider when backend config changes (e.g., after model selection or initial load)
    useEffect(() => {
        setUiProvider(transcriptModelConfig.provider);
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

    useEffect(() => {
        if (!cloudTranscriptionEnabled && isCloudProvider) {
            setUiProvider('parakeet');
        }
    }, [cloudTranscriptionEnabled, isCloudProvider]);

    const fetchApiKey = async (provider: string) => {
        try {

            const data = await invoke('api_get_transcript_api_key', { provider }) as string;

            setApiKey(data || '');
        } catch (err) {
            console.error('Error fetching API key:', err);
            setApiKey(null);
        }
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
                baseUrl: nextConfig.baseUrl,
                endpoint: nextConfig.endpoint,
                region: nextConfig.region,
            });
            setTranscriptModelConfig(nextConfig);
            toast.success('Transcription settings saved');
            if (onModelSelect) {
                onModelSelect();
            }
        } catch (err) {
            console.error('Failed to save cloud transcription settings:', err);
            toast.error('Failed to save transcription settings');
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
                        name: 'Audio and video',
                        extensions: ['aac', 'flac', 'm4a', 'mkv', 'mp3', 'mp4', 'ogg', 'opus', 'wav', 'webm', 'wma'],
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
                apiKey: nextConfig.apiKey,
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

    return (
        <div>
            <div>
                {/* <div className="flex justify-between items-center mb-4">
                    <h3 className="text-lg font-semibold text-foreground">Transcript Settings</h3>
                </div> */}
                <div className="space-y-4 pb-6">
                    <div>
                        <Label className="block text-sm font-medium text-foreground mb-1">
                            Engine
                        </Label>
                        <div className="flex space-x-2 mx-1">
                            <Select
                                value={uiProvider}
                                onValueChange={(value) => {
                                    const provider = value as TranscriptModelProps['provider'];
                                    setUiProvider(provider);
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
                                <SelectTrigger className='focus:ring-1 focus:ring-ring focus:border-primary'>
                                    <SelectValue placeholder="Select provider" />
                                </SelectTrigger>
                                <SelectContent>
                                    <SelectGroup>
                                        <SelectLabel>On your device</SelectLabel>
                                        <SelectItem value="parakeet">Parakeet &middot; real-time, recommended</SelectItem>
                                        <SelectItem value="localWhisper">Whisper &middot; highest accuracy</SelectItem>
                                        <SelectItem value="nemotron">Nemotron &middot; streaming, multilingual (beta)</SelectItem>
                                    </SelectGroup>
                                    {cloudTranscriptionEnabled && (
                                        <SelectGroup>
                                            <SelectLabel>Cloud APIs</SelectLabel>
                                            <SelectItem value="cloud-whisper">Hosted Whisper &middot; OpenAI-compatible</SelectItem>
                                            <SelectItem value="mai-transcribe">MAI-Transcribe &middot; Azure Speech</SelectItem>
                                        </SelectGroup>
                                    )}
                                </SelectContent>
                            </Select>

                            {uiProvider !== 'localWhisper' && uiProvider !== 'parakeet' && uiProvider !== 'nemotron' && !isCloudProvider && (
                                <Select
                                    value={transcriptModelConfig.model}
                                    onValueChange={(value) => {
                                        const model = value as TranscriptModelProps['model'];
                                        setTranscriptModelConfig({ ...transcriptModelConfig, provider: uiProvider, model });
                                    }}
                                >
                                    <SelectTrigger className='focus:ring-1 focus:ring-ring focus:border-primary'>
                                        <SelectValue placeholder="Select model" />
                                    </SelectTrigger>
                                    <SelectContent>
                                        {modelOptions[uiProvider].map((model) => (
                                            <SelectItem key={model} value={model}>{model}</SelectItem>
                                        ))}
                                    </SelectContent>
                                </Select>
                            )}

                        </div>
                    </div>

                    {uiProvider === 'localWhisper' && (
                        <div className="mt-6 space-y-4">
                            <WhisperAccelerationStatus />
                            <ModelManager
                                selectedModel={transcriptModelConfig.provider === 'localWhisper' ? transcriptModelConfig.model : undefined}
                                onModelSelect={handleWhisperModelSelect}
                                autoSave={true}
                            />
                        </div>
                    )}

                    {uiProvider === 'parakeet' && (
                        <div className="mt-6">
                            <ParakeetModelManager
                                selectedModel={transcriptModelConfig.provider === 'parakeet' ? transcriptModelConfig.model : undefined}
                                onModelSelect={handleParakeetModelSelect}
                                autoSave={true}
                            />
                        </div>
                    )}

                    {uiProvider === 'nemotron' && (
                        <div className="mt-6">
                            <NemotronModelManager
                                selectedModel={transcriptModelConfig.provider === 'nemotron' ? transcriptModelConfig.model : undefined}
                                onModelSelect={handleNemotronModelSelect}
                                autoSave={true}
                            />
                        </div>
                    )}

                    {uiProvider === 'cloud-whisper' && cloudTranscriptionEnabled && (
                        <div className="mt-6 space-y-4">
                            <div>
                                <Label className="block text-sm font-medium text-foreground mb-1">
                                    Base URL
                                </Label>
                                <Input
                                    className="mx-1 focus:ring-1 focus:ring-ring focus:border-primary"
                                    value={cloudWhisperBaseUrl}
                                    onChange={(e) => setCloudWhisperBaseUrl(e.target.value)}
                                    placeholder="https://api.openai.com/v1"
                                />
                            </div>
                            <div>
                                <Label className="block text-sm font-medium text-foreground mb-1">
                                    Model
                                </Label>
                                <Input
                                    className="mx-1 focus:ring-1 focus:ring-ring focus:border-primary"
                                    value={cloudModel}
                                    onChange={(e) => setCloudModel(e.target.value)}
                                    placeholder="whisper-1"
                                />
                            </div>
                            <p className="mx-1 rounded-md border border-border bg-muted px-3 py-2 text-xs text-muted-foreground">
                                OpenAI file transcription uploads are limited to 25 MB.
                                Larger recordings fall back to local transcription.
                            </p>
                        </div>
                    )}

                    {uiProvider === 'mai-transcribe' && cloudTranscriptionEnabled && (
                        <div className="mt-6 space-y-4">
                            <div>
                                <Label className="block text-sm font-medium text-foreground mb-1">
                                    Azure Speech endpoint
                                </Label>
                                <Input
                                    className="mx-1 focus:ring-1 focus:ring-ring focus:border-primary"
                                    value={maiEndpoint}
                                    onChange={(e) => setMaiEndpoint(e.target.value)}
                                    placeholder="https://your-resource.cognitiveservices.azure.com"
                                />
                            </div>
                            <div>
                                <Label className="block text-sm font-medium text-foreground mb-1">
                                    Region
                                </Label>
                                <Input
                                    className="mx-1 focus:ring-1 focus:ring-ring focus:border-primary"
                                    value={maiRegion}
                                    onChange={(e) => setMaiRegion(e.target.value)}
                                    placeholder="eastus"
                                />
                            </div>
                            <div>
                                <Label className="block text-sm font-medium text-foreground mb-1">
                                    Model
                                </Label>
                                <Input
                                    className="mx-1 focus:ring-1 focus:ring-ring focus:border-primary"
                                    value={cloudModel}
                                    onChange={(e) => setCloudModel(e.target.value)}
                                    placeholder="mai-transcribe-1.5"
                                />
                            </div>
                            <p className="mx-1 rounded-md border border-border bg-muted px-3 py-2 text-xs text-muted-foreground">
                                This model provides sentence-level timing only; speaker
                                splitting will be less precise.
                            </p>
                        </div>
                    )}


                    {requiresApiKey && (
                        <div>
                            <Label className="block text-sm font-medium text-foreground mb-1">
                                API Key
                            </Label>
                            <div className="relative mx-1">
                                <Input
                                    type={showApiKey ? "text" : "password"}
                                    className={`pr-24 focus:ring-1 focus:ring-ring focus:border-primary ${isApiKeyLocked ? 'bg-muted cursor-not-allowed' : ''
                                        }`}
                                    value={apiKey || ''}
                                    onChange={(e) => setApiKey(e.target.value)}
                                    disabled={isApiKeyLocked}
                                    onClick={handleInputClick}
                                    placeholder="Enter your API key"
                                />
                                {isApiKeyLocked && (
                                    <div
                                        onClick={handleInputClick}
                                        className="absolute inset-0 flex items-center justify-center bg-muted bg-opacity-50 rounded-md cursor-not-allowed"
                                    />
                                )}
                                <div className="absolute inset-y-0 right-0 pr-1 flex items-center">
                                    <Button
                                        type="button"
                                        variant="ghost"
                                        size="icon"
                                        onClick={() => setIsApiKeyLocked(!isApiKeyLocked)}
                                        className={`transition-colors duration-200 ${isLockButtonVibrating ? 'animate-vibrate text-red-500' : ''
                                            }`}
                                        title={isApiKeyLocked ? "Unlock to edit" : "Lock to prevent editing"}
                                    >
                                        {isApiKeyLocked ? <Lock className="h-4 w-4" /> : <Unlock className="h-4 w-4" />}
                                    </Button>
                                    <Button
                                        type="button"
                                        variant="ghost"
                                        size="icon"
                                        onClick={() => setShowApiKey(!showApiKey)}
                                    >
                                        {showApiKey ? <EyeOff className="h-4 w-4" /> : <Eye className="h-4 w-4" />}
                                    </Button>
                                </div>
                            </div>
                        </div>
                    )}

                    {isCloudProvider && cloudTranscriptionEnabled && (
                        <div className="flex justify-end gap-2">
                            <Button
                                type="button"
                                variant="gray"
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
                            <Button type="button" onClick={handleSaveCloudProvider} disabled={isTestingProvider}>
                                Save
                            </Button>
                        </div>
                    )}
                </div>
            </div>
        </div >
    )
}








