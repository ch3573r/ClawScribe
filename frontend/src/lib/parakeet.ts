// Types for Parakeet (NVIDIA NeMo) integration
export interface ParakeetModelInfo {
  name: string;
  path: string;
  size_mb: number;
  accuracy: ModelAccuracy;
  speed: ProcessingSpeed;
  status: ModelStatus;
  description?: string;
  quantization: QuantizationType;
}

export type QuantizationType = 'FP32' | 'FP16' | 'Int8';
export type ModelAccuracy = 'High' | 'Good' | 'Decent';
export type ProcessingSpeed = 'Slow' | 'Medium' | 'Fast' | 'Very Fast' | 'Ultra Fast';

export type ModelStatus =
  | 'Available'
  | 'Missing'
  | { Downloading: number }
  | { Error: string }
  | { Corrupted: { file_size: number; expected_min_size: number } };

export interface ParakeetEngineState {
  currentModel: string | null;
  availableModels: ParakeetModelInfo[];
  isLoading: boolean;
  error: string | null;
}

// User-friendly model display configuration
export interface ModelDisplayInfo {
  friendlyName: string;
  /** Short "Best for" text shown in the model table. */
  bestFor: string;
  /** Quiet tag shown next to the name. */
  badge?: 'Recommended' | 'Experimental';
  recommended?: boolean;
  tier: 'fastest' | 'balanced' | 'precise';
}

export const MODEL_DISPLAY_CONFIG: Record<string, ModelDisplayInfo> = {
  'parakeet-tdt-0.6b-v3-int8': {
    friendlyName: 'Lightning',
    bestFor: 'Everyday meetings, fastest',
    badge: 'Recommended',
    recommended: true,
    tier: 'fastest'
  },
  'parakeet-tdt-0.6b-v3-smoothquant-int8': {
    friendlyName: 'SmoothQuant',
    bestFor: 'Long recordings',
    badge: 'Experimental',
    tier: 'balanced'
  },
  'parakeet-tdt-0.6b-ultra-int8': {
    friendlyName: 'Parakeet Ultra',
    bestFor: 'German and English accuracy',
    tier: 'precise'
  },
  'parakeet-tdt-0.6b-ultra-fp16': {
    friendlyName: 'Parakeet Ultra fp16',
    bestFor: 'Highest precision',
    tier: 'precise'
  },
  'parakeet-tdt-0.6b-v2-int8': {
    friendlyName: 'Compact',
    bestFor: 'Smaller download, good accuracy',
    tier: 'balanced'
  }
};

// Model configuration for Parakeet models (matching Rust implementation)
// Supported models: parakeet-tdt-0.6b in v2 and v3 variants
// Sources: istupakov v2/v3 ONNX and Olicorne SmoothQuant variants.
export const PARAKEET_MODEL_CONFIGS: Record<string, Partial<ParakeetModelInfo>> = {
  'parakeet-tdt-0.6b-v3-int8': {
    description: 'Fastest default. Stock v3 int8; measured around 22x realtime on the 9070 XT.',
    size_mb: 670, // Actual download: 652MB encoder + 18.2MB decoder + 0.2MB extras
    accuracy: 'High',
    speed: 'Ultra Fast',
    quantization: 'Int8'
  },
  'parakeet-tdt-0.6b-v3-smoothquant-int8': {
    description: 'Experimental SmoothQuant int8 export aimed at better long-audio accuracy than stock int8.',
    size_mb: 813,
    accuracy: 'High',
    speed: 'Very Fast',
    quantization: 'Int8'
  },
  'parakeet-tdt-0.6b-ultra-int8': {
    description: 'Fine-tuned v3 with better accuracy in German and English. 25 languages.',
    size_mb: 671,
    accuracy: 'High',
    speed: 'Fast',
    quantization: 'Int8'
  },
  'parakeet-tdt-0.6b-ultra-fp16': {
    description: 'Fine-tuned v3 at higher precision, with better accuracy in German and English. 25 languages. Larger download.',
    size_mb: 1276,
    accuracy: 'High',
    speed: 'Fast',
    quantization: 'FP16'
  },
  'parakeet-tdt-0.6b-v2-int8': {
    description: '25x real-time, smaller size with good accuracy',
    size_mb: 661, // Actual download: 652MB encoder + 9MB decoder + 0.15MB extras
    accuracy: 'High',
    speed: 'Very Fast',
    quantization: 'Int8'
  }
};

// Helper functions
// Get user-friendly display name for a model
export function getModelDisplayName(modelName: string): string {
  const displayInfo = MODEL_DISPLAY_CONFIG[modelName];
  return displayInfo?.friendlyName || modelName;
}

// Get model display info (name, best-for text, badge)
export function getModelDisplayInfo(modelName: string): ModelDisplayInfo | null {
  return MODEL_DISPLAY_CONFIG[modelName] || null;
}

export function getStatusColor(status: ModelStatus): string {
  if (status === 'Available') return 'green';
  if (status === 'Missing') return 'gray';
  if (typeof status === 'object' && 'Downloading' in status) return 'blue';
  if (typeof status === 'object' && 'Error' in status) return 'red';
  return 'gray';
}

export function formatFileSize(sizeMb: number): string {
  if (sizeMb >= 1000) {
    return `${(sizeMb / 1000).toFixed(1)}GB`;
  }
  return `${sizeMb}MB`;
}

// Download size as shown in the model table, for example "1,276 MB".
export function formatModelSize(sizeMb: number): string {
  return `${Math.round(sizeMb).toLocaleString('en-US')} MB`;
}

// Helper function to check if model is quantized
export function isQuantizedModel(modelName: string): boolean {
  return modelName.includes('int8');
}

// Helper function to get model performance badge
export function getModelPerformanceBadge(quantization: QuantizationType): { label: string; color: string } {
  switch (quantization) {
    case 'FP32':
      return { label: 'Full Precision', color: 'blue' };
    case 'FP16':
      return { label: 'FP16', color: 'blue' };
    case 'Int8':
      return { label: 'Int8 Quantized', color: 'green' };
    default:
      return { label: 'Standard', color: 'gray' };
  }
}

export function getRecommendedModel(systemSpecs?: { ram: number; cores: number }): string {
  // Default to Int8 quantized model (fastest)
  if (!systemSpecs) return 'parakeet-tdt-0.6b-v3-int8';

  // For any system, prefer Int8 for speed
  // FP32 can be used if user explicitly wants higher precision
  return 'parakeet-tdt-0.6b-v3-int8';
}

// Tauri command wrappers for Parakeet backend
import { invoke } from '@tauri-apps/api/core';

export class ParakeetAPI {
  static async init(): Promise<void> {
    await invoke('parakeet_init');
  }

  static async getAvailableModels(): Promise<ParakeetModelInfo[]> {
    return await invoke('parakeet_get_available_models');
  }

  static async loadModel(modelName: string): Promise<void> {
    await invoke('parakeet_load_model', { modelName });
  }

  static async getCurrentModel(): Promise<string | null> {
    return await invoke('parakeet_get_current_model');
  }

  static async isModelLoaded(): Promise<boolean> {
    return await invoke('parakeet_is_model_loaded');
  }

  static async transcribeAudio(audioData: number[]): Promise<string> {
    return await invoke('parakeet_transcribe_audio', { audioData });
  }

  static async getModelsDirectory(): Promise<string> {
    return await invoke('parakeet_get_models_directory');
  }

  static async downloadModel(modelName: string): Promise<void> {
    await invoke('parakeet_download_model', { modelName });
  }

  static async cancelDownload(modelName: string): Promise<void> {
    await invoke('parakeet_cancel_download', { modelName });
  }

  static async deleteCorruptedModel(modelName: string): Promise<string> {
    return await invoke('parakeet_delete_corrupted_model', { modelName });
  }

  static async hasAvailableModels(): Promise<boolean> {
    return await invoke('parakeet_has_available_models');
  }

  static async validateModelReady(): Promise<string> {
    return await invoke('parakeet_validate_model_ready');
  }

  static async openModelsFolder(): Promise<void> {
    await invoke('open_parakeet_models_folder');
  }
}
