# Transcription speed samples

These original, fictional meeting excerpts were synthesized offline using
eSpeak NG 1.52.0's built-in German (`de`) and American English (`en-us`)
formant voices at 145 words per minute. Only the resulting audio is bundled.
[eSpeak NG](https://github.com/espeak-ng/espeak-ng)

- German, 15.065 seconds. “Guten Morgen. Heute planen wir die nächsten
  Schritte für unser Projekt. Am Mittwoch sprechen wir mit dem Team über die
  offenen Fragen. Danach schreiben wir die Ergebnisse auf und vereinbaren einen
  Termin für die nächste Woche.”
- English, 14.651 seconds. “Good morning. Today we are planning the next steps
  for our project. On Wednesday we will meet with the team to discuss the open
  questions. Afterwards we will write down the results and arrange a meeting
  for next week.”

Generate a WAV with `espeak-ng -v <voice> -s 145 -w sample.wav "<text>"`,
then convert it to headerless little-endian, mono, 16 kHz PCM16 with
`ffmpeg -i sample.wav -ar 16000 -ac 1 -f s16le <language>.s16le`.
