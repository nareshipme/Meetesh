# Speech fixtures for the transcription quality tests

Six synthetic sentences (espeak-ng, 16 kHz mono 16-bit). The text is the ground truth for the word
error rate in `tests/asr_quality.rs`. **This is synthetic speech, not real speech**: it exercises the
audio handling (volume, noise, pauses, chunking) but does not say how accurate Whisper is on real voices.

| File | Voice | Text |
|---|---|---|
| s1.wav | en-us | Let's start with the launch date. Engineering thinks the fifteenth is realistic. |
| s2.wav | en-us, pitch 75 | Support wants another week to finish the training material. |
| s3.wav | en-us | Priya, can you confirm who owns the pricing page? |
| s4.wav | en-us, pitch 75 | I will send the revised budget to finance by Friday afternoon. |
| s5.wav | en-us | We agreed to review the contract with legal before signing anything. |
| s6.wav | en-us, pitch 75 | Please add the customer feedback to the shared document tonight. |

Regenerate (needs `espeak-ng` and `ffmpeg`):

```bash
espeak-ng -v en-us [-p 75] -s 150 -w out.wav "TEXT" && ffmpeg -y -i out.wav -ar 16000 -ac 1 -sample_fmt s16 sN.wav
```
