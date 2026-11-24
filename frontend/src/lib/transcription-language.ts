import { LANGUAGES } from '@/constants/languages';

/**
 * How the language setting applies to the selected transcription engine. This
 * mirrors the live engine choice in the core (`get_or_init_transcription_engine`).
 * - whisper: live recording uses it; Auto detect and translate-to-English work.
 *   Every provider id the core does not know (the old deepgram, elevenLabs, groq
 *   and openai ids) also runs Whisper.
 * - nemotron: live recording and batch jobs use it as a prompt. It cannot detect
 *   a language or translate; Auto means the system language.
 * - detects: Parakeet finds the language itself and ignores the setting. It is
 *   also what the core uses when no engine config is saved at all (no provider
 *   here). A saved config with a blank provider is not that case: the core runs
 *   Whisper for it, so '' maps to whisper.
 * - hosted: the hosted engines (cloud-whisper, mai-transcribe) are batch only.
 *   Live recording uses Parakeet; the language of an import or retranscription is
 *   chosen in that dialog.
 */
export type LanguageSupport = 'whisper' | 'nemotron' | 'detects' | 'hosted';

export function languageSupport(provider: string | null | undefined): LanguageSupport {
  switch (provider) {
    case null:
    case undefined:
    case 'parakeet':
      return 'detects';
    case 'nemotron': return 'nemotron';
    case 'cloud-whisper':
    case 'mai-transcribe':
      return 'hosted';
    default: return 'whisper';
  }
}

export const isAutoLanguage = (code: string) => code === 'auto' || code === 'auto-translate';

/** Standard Azure Speech transcribes audio in its original language. */
export function fileLanguageOptions(provider?: string | null) {
  return LANGUAGES.filter(option => provider !== 'mai-transcribe' || option.code !== 'auto-translate');
}

export interface LanguageOption { code: string; label: string }

export function languageLabel(code: string, name: string, support: LanguageSupport = 'whisper'): string {
  if (code === 'auto') return support === 'nemotron' ? 'System language' : 'Auto detect';
  if (code === 'auto-translate') return 'Auto detect, translate to English';
  return name;
}

/** The choices for an engine, or none when the engine ignores the setting. */
export function languageOptions(support: LanguageSupport): LanguageOption[] {
  if (support === 'detects' || support === 'hosted') return [];
  return LANGUAGES
    .filter(language => support === 'whisper' || language.code !== 'auto-translate')
    .map(language => ({ code: language.code, label: languageLabel(language.code, language.name, support) }));
}

/** The label of any stored value, whether or not the current engine offers it. */
export function storedLanguageLabel(code: string): string {
  const language = LANGUAGES.find(item => item.code === code);
  return languageLabel(code, language?.name ?? code);
}
