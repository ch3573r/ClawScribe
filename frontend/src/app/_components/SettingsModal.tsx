import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { ModelConfig } from "@/components/ModelSettingsModal";
import { PreferenceSettings } from "@/components/PreferenceSettings";
import { KnowledgeSettings } from "@/components/KnowledgeSettings";
import { DeviceSelection } from "@/components/DeviceSelection";
import { LanguageSelection } from "@/components/LanguageSelection";
import { TranscriptSettings } from "@/components/TranscriptSettings";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { toast } from "sonner";
import { useConfig } from "@/contexts/ConfigContext";
import { useRecordingState } from "@/contexts/RecordingStateContext";

const EMPTY_MODEL_OPTION = "__empty_model__";
const PROVIDER_OPTIONS: ModelConfig["provider"][] = [
  "custom-openai", "openclaw", "codex", "builtin-ai", "ollama", "openrouter", "claude", "groq",
];

type modalType = "modelSettings" | "deviceSettings" | "languageSettings" | "modelSelector" | "errorAlert" | "chunkDropWarning";

/**
 * SettingsModals Component
 *
 * All settings modals consolidated into a single component.
 * Uses ConfigContext and RecordingStateContext internally - no prop drilling needed!
 */

interface SettingsModalsProps {
  modals: {
    modelSettings: boolean;
    deviceSettings: boolean;
    languageSettings: boolean;
    modelSelector: boolean;
    errorAlert: boolean;
    chunkDropWarning: boolean;
  };
  messages: {
    errorAlert: string;
    chunkDropWarning: string;
    modelSelector: string;
  };
  onClose: (name: modalType) => void;
}

export function SettingsModals({
  modals,
  messages,
  onClose,
}: SettingsModalsProps) {
  // Contexts
  const {
    modelConfig,
    setModelConfig,
    models,
    modelOptions,
    error,
    selectedDevices,
    setSelectedDevices,
    selectedLanguage,
    setSelectedLanguage,
    transcriptModelConfig,
    setTranscriptModelConfig,
    showConfidenceIndicator,
    toggleConfidenceIndicator,
  } = useConfig();

  const { isRecording } = useRecordingState();

  const providerSelectValue = PROVIDER_OPTIONS.includes(modelConfig.provider)
    ? modelConfig.provider
    : PROVIDER_OPTIONS[0];
  const availableModels = modals.modelSettings ? modelOptions[modelConfig.provider] : [];
  const modelSelectOption = availableModels.includes(modelConfig.model)
    ? modelConfig.model
    : availableModels[0];
  const modelSelectValue = modelSelectOption === ""
    ? EMPTY_MODEL_OPTION
    : modelSelectOption ?? "";

  return <>
    {/* Legacy Settings Modal */}
    {modals.modelSettings && (
      <div className="fixed inset-0 bg-overlay/50 flex items-center justify-center z-50 p-4">
        <div className="bg-card rounded-lg shadow-xl max-w-4xl w-full max-h-[90vh] overflow-hidden flex flex-col">
          {/* Header */}
          <div className="flex justify-between items-center p-6 border-b">
            <h3 className="text-xl font-semibold text-foreground">Preferences</h3>
            <Button
              variant="ghost"
              aria-label="Close preferences"
              onClick={() => onClose("modelSettings")
              }
              className="h-auto whitespace-normal p-0 font-normal hover:bg-transparent text-muted-foreground hover:text-foreground transition-colors"
            >
              <svg xmlns="http://www.w3.org/2000/svg" className="!h-6 !w-6" fill="none" viewBox="0 0 24 24" stroke="currentColor">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M6 18L18 6M6 6l12 12" />
              </svg>
            </Button>
          </div>

          {/* Content - Scrollable */}
          <div className="flex-1 overflow-y-auto p-6 space-y-8">
            {/* General Preferences Section */}
            <PreferenceSettings />
            <KnowledgeSettings />

            {/* Divider */}
            <div className="border-t pt-8">
              <h4 className="text-lg font-semibold text-foreground mb-4">AI model configuration</h4>
              <div className="space-y-4">
                <div>
                  <label className="block text-sm font-medium text-foreground mb-1">
                    Summarization model
                  </label>
                  <div className="flex space-x-2">
                    <Select
                      value={providerSelectValue}
                      onValueChange={(value) => {
                        const provider = value as ModelConfig['provider'];
                        setModelConfig({
                          ...modelConfig,
                          provider,
                          model: modelOptions[provider][0]
                        });
                      }}
                    >
                      <SelectTrigger aria-label="Summarization provider" className="h-auto w-auto px-3 py-2 text-sm bg-card border border-input rounded-md shadow-sm focus:outline-none focus:ring-1 focus:ring-ring focus:border-ring">
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        <SelectItem value="custom-openai">OpenAI / OpenAI-compatible API</SelectItem>
                        <SelectItem value="openclaw">OpenClaw</SelectItem>
                        <SelectItem value="codex">Advanced: Codex app-server</SelectItem>
                        <SelectItem value="builtin-ai">Built-in AI</SelectItem>
                        <SelectItem value="ollama">Ollama</SelectItem>
                        <SelectItem value="openrouter">OpenRouter</SelectItem>
                        <SelectItem value="claude">Claude</SelectItem>
                        <SelectItem value="groq">Groq</SelectItem>
                      </SelectContent>
                    </Select>

                    <Select
                      value={modelSelectValue}
                      onValueChange={(value) => setModelConfig((prev: ModelConfig) => ({ ...prev, model: value === EMPTY_MODEL_OPTION ? "" : value }))}
                    >
                      <SelectTrigger aria-label="Summarization model" className="h-auto flex-1 px-3 py-2 text-sm bg-card border border-input rounded-md shadow-sm focus:outline-none focus:ring-1 focus:ring-ring focus:border-ring">
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        {modelOptions[modelConfig.provider].map((model: string) => (
                          <SelectItem key={model} value={model || EMPTY_MODEL_OPTION}>
                            {model}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                  </div>
                </div>
                {modelConfig.provider === 'ollama' && (
                  <div>
                    <h4 className="text-lg font-bold mb-4">Available Ollama models</h4>
                    {error && (
                      <div className="bg-error border border-error-border/30 text-error-foreground px-4 py-3 rounded mb-4">
                        {error}
                      </div>
                    )}
                    <div className="grid gap-4 max-h-[400px] overflow-y-auto pr-2">
                      {models.map((model) => (
                        <div
                          key={model.id}
                          className={`bg-card p-4 rounded-lg shadow cursor-pointer transition-colors ${modelConfig.model === model.name ? 'ring-2 ring-primary bg-primary/10' : 'hover:bg-muted'
                            }`}
                          onClick={() => setModelConfig((prev: ModelConfig) => ({ ...prev, model: model.name }))}
                        >
                          <h3 className="font-semibold">{model.name}</h3>
                          <p className="text-muted-foreground">Size: <span className="font-mono text-foreground">{model.size}</span></p>
                          <p className="text-muted-foreground">Modified: <span className="font-mono text-foreground">{model.modified}</span></p>
                        </div>
                      ))}
                    </div>
                  </div>
                )}
              </div>
            </div>
          </div>

          {/* Footer */}
          <div className="border-t p-6 flex justify-end">
            <Button
              variant="ghost"
              onClick={() => onClose('modelSettings')}
              className="h-auto whitespace-normal px-4 py-2 text-sm font-medium text-primary-foreground bg-primary rounded-md hover:bg-primary/90 focus:outline-none focus:ring-2 focus:ring-offset-2 focus:ring-ring transition-colors hover:text-primary-foreground"
            >
              Done
            </Button>
          </div>
        </div>
      </div>
    )}

    {/* Device Settings Modal */}
    {modals.deviceSettings && (
      <div className="fixed inset-0 bg-overlay/50 flex items-center justify-center z-50">
        <div className="bg-card rounded-lg p-6 max-w-md w-full mx-4 shadow-xl">
          <div className="flex justify-between items-center mb-4">
            <h3 className="text-lg font-semibold text-foreground">Audio device settings</h3>
            <Button
              variant="ghost"
              aria-label="Close audio device settings"
              onClick={() => onClose('deviceSettings')}
              className="h-auto whitespace-normal p-0 font-normal hover:bg-transparent text-muted-foreground hover:text-foreground transition-colors"
            >
              <svg xmlns="http://www.w3.org/2000/svg" className="!h-6 !w-6" fill="none" viewBox="0 0 24 24" stroke="currentColor">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M6 18L18 6M6 6l12 12" />
              </svg>
            </Button>
          </div>

          <DeviceSelection
            selectedDevices={selectedDevices}
            onDeviceChange={setSelectedDevices}
            disabled={isRecording}
          />

          <div className="mt-6 flex justify-end">
            <Button
              variant="ghost"
              onClick={() => {
                const micDevice = selectedDevices.micDevice || 'Default';
                const systemDevice = selectedDevices.systemDevice || 'Default';
                toast.success("Devices selected", {
                  description: `Microphone: ${micDevice}, System Audio: ${systemDevice}`
                });
                onClose('deviceSettings');
              }}
              className="h-auto whitespace-normal px-4 py-2 text-sm font-medium text-primary-foreground bg-primary rounded-md hover:bg-primary/90 focus:outline-none focus:ring-2 focus:ring-offset-2 focus:ring-ring transition-colors hover:text-primary-foreground"
            >
              Done
            </Button>
          </div>
        </div>
      </div>
    )}

    {/* Language settings Modal */}
    {modals.languageSettings && (
      <div className="fixed inset-0 bg-overlay/50 flex items-center justify-center z-50">
        <div className="bg-card rounded-lg p-6 max-w-md w-full mx-4 shadow-xl">
          <div className="flex justify-between items-center mb-4">
            <h3 className="text-lg font-semibold text-foreground">Language settings</h3>
            <Button
              variant="ghost"
              aria-label="Close language settings"
              onClick={() => onClose('languageSettings')}
              className="h-auto whitespace-normal p-0 font-normal hover:bg-transparent text-muted-foreground hover:text-foreground transition-colors"
            >
              <svg xmlns="http://www.w3.org/2000/svg" className="!h-6 !w-6" fill="none" viewBox="0 0 24 24" stroke="currentColor">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M6 18L18 6M6 6l12 12" />
              </svg>
            </Button>
          </div>

          <LanguageSelection
            selectedLanguage={selectedLanguage}
            onLanguageChange={setSelectedLanguage}
            disabled={isRecording}
            provider={transcriptModelConfig.provider}
          />

          <div className="mt-6 flex justify-end">
            <Button
              variant="ghost"
              onClick={() => onClose('languageSettings')}
              className="h-auto whitespace-normal px-4 py-2 text-sm font-medium text-primary-foreground bg-primary rounded-md hover:bg-primary/90 focus:outline-none focus:ring-2 focus:ring-offset-2 focus:ring-ring transition-colors hover:text-primary-foreground"
            >
              Done
            </Button>
          </div>
        </div>
      </div>
    )}

    {/* Model Selection Modal */}
    {modals.modelSelector && (
      <div className="fixed inset-0 bg-overlay/50 flex items-center justify-center z-50">
        <div className="bg-card rounded-lg max-w-4xl w-full mx-4 shadow-xl max-h-[90vh] flex flex-col">
          {/* Fixed Header */}
          <div className="flex justify-between items-center p-6 pb-4 border-b border-border">
            <h3 className="text-lg font-semibold text-foreground">
              {messages.modelSelector ? 'Speech recognition setup required' : 'Transcription model settings'}
            </h3>
            <Button
              variant="ghost"
              aria-label="Close transcription model settings"
              onClick={() => onClose('modelSelector')}
              className="h-auto whitespace-normal p-0 font-normal hover:bg-transparent text-muted-foreground hover:text-foreground transition-colors"
            >
              <svg xmlns="http://www.w3.org/2000/svg" className="!h-6 !w-6" fill="none" viewBox="0 0 24 24" stroke="currentColor">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M6 18L18 6M6 6l12 12" />
              </svg>
            </Button>
          </div>

          {/* Scrollable Content */}
          <div className="flex-1 overflow-y-auto p-6 pt-4">
            <TranscriptSettings
              transcriptModelConfig={transcriptModelConfig}
              setTranscriptModelConfig={setTranscriptModelConfig}
              onModelSelect={() => onClose('modelSelector')}
            />
          </div>

          {/* Fixed Footer */}
          <div className="p-6 pt-4 border-t border-border flex items-center justify-between">
            {/* Confidence Indicator Toggle */}
            <div className="flex items-center gap-3">
              <label className="relative inline-flex items-center cursor-pointer">
                <Switch
                  checked={showConfidenceIndicator}
                  onCheckedChange={toggleConfidenceIndicator}
                  aria-label="Show confidence indicators"
                  className="w-11 h-6 border-0 shadow-none data-[state=unchecked]:bg-input [&>span]:h-5 [&>span]:w-5 [&>span]:shadow-none [&>span]:border [&>span]:border-border [&>span]:data-[state=checked]:border-background [&>span]:data-[state=checked]:translate-x-[22px] [&>span]:data-[state=unchecked]:translate-x-0.5 rtl:[&>span]:data-[state=checked]:-translate-x-[22px] rtl:[&>span]:data-[state=unchecked]:-translate-x-0.5"
                />
              </label>
              <div>
                <p className="text-sm font-medium text-foreground">Show confidence indicators</p>
                <p className="text-xs text-muted-foreground">Display colored dots showing transcription confidence quality</p>
              </div>
            </div>

            <Button
              variant="ghost"
              onClick={() => onClose('modelSelector')}
              className="h-auto whitespace-normal px-4 py-2 text-sm font-medium text-foreground bg-secondary rounded-md hover:bg-secondary/80 focus:outline-none focus:ring-2 focus:ring-offset-2 focus:ring-ring transition-colors hover:text-foreground"
            >
              {messages.modelSelector ? 'Cancel' : 'Done'}
            </Button>
          </div>
        </div>
      </div>
    )}

    {/* Error Alert Modal */}
    {modals.errorAlert && (
      <div className="fixed inset-0 bg-overlay/50 flex items-center justify-center z-50">
        <Alert className="max-w-md mx-4 border-error-border/30 bg-card shadow-xl">
          <AlertTitle className="text-error-foreground">Recording stopped</AlertTitle>
          <AlertDescription className="text-error-foreground">
            {messages.errorAlert}
            <Button
              variant="ghost"
              onClick={() => onClose('errorAlert')}
              className="h-auto whitespace-normal p-0 font-normal hover:bg-transparent ml-2 text-error-foreground hover:text-error-foreground underline"
            >
              Dismiss
            </Button>
          </AlertDescription>
        </Alert>
      </div>
    )}

    {/* Chunk Drop Warning Modal */}
    {modals.chunkDropWarning && (
      <div className="fixed inset-0 bg-overlay/50 flex items-center justify-center z-50">
        <Alert className="max-w-lg mx-4 border-warning-border/30 bg-card shadow-xl">
          <AlertTitle className="text-warning-foreground">Transcription performance warning</AlertTitle>
          <AlertDescription className="text-warning-foreground">
            {messages.chunkDropWarning}
            <Button
              variant="ghost"
              onClick={() => onClose('chunkDropWarning')}
              className="h-auto whitespace-normal p-0 font-normal hover:bg-transparent ml-2 text-warning-foreground hover:text-warning-foreground underline"
            >
              Dismiss
            </Button>
          </AlertDescription>
        </Alert>
      </div>
    )}
  </>
}
