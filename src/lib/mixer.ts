/**
 * Микшер дорожек для просмотра.
 *
 * WebView2 играет из видео только первую звуковую дорожку, поэтому каждая дорожка вынесена
 * в отдельный .m4a (см. extract_audio_tracks) и играет своим <audio>, ведомым видео:
 * пуск/пауза/перемотка повторяют видео, а раз в кадр проверяется расхождение — если
 * больше 60 мс, время дорожки выравнивается. Громкость (до 200%) — через GainNode Web Audio.
 */
export class Mixer {
  private ctx = new AudioContext();
  private master = this.ctx.createGain();
  private els: HTMLAudioElement[] = [];
  private gains: GainNode[] = [];
  private lastSync = 0;

  constructor(urls: string[]) {
    this.master.connect(this.ctx.destination);
    for (const url of urls) {
      const el = new Audio();
      el.crossOrigin = "anonymous"; // иначе Web Audio отдаст тишину для другого источника
      el.preload = "auto";
      el.src = url;
      const gain = this.ctx.createGain();
      this.ctx.createMediaElementSource(el).connect(gain).connect(this.master);
      this.els.push(el);
      this.gains.push(gain);
    }
  }

  setTrack(i: number, enabled: boolean, volume: number) {
    const g = this.gains[i];
    if (g) g.gain.value = enabled ? Math.max(0, Math.min(2, volume)) : 0;
  }

  setMaster(volume: number, muted: boolean) {
    this.master.gain.value = muted ? 0 : volume;
  }

  setRate(rate: number) {
    for (const el of this.els) el.playbackRate = rate;
  }

  play(t: number) {
    void this.ctx.resume();
    for (const el of this.els) {
      el.currentTime = t;
      void el.play().catch(() => {});
    }
  }

  pause(t?: number) {
    for (const el of this.els) {
      el.pause();
      if (t !== undefined) el.currentTime = t;
    }
  }

  seek(t: number) {
    for (const el of this.els) el.currentTime = t;
  }

  /** Вызывается во время воспроизведения; не чаще 4 раз в секунду проверяет расхождение. */
  sync(t: number) {
    const now = performance.now();
    if (now - this.lastSync < 250) return;
    this.lastSync = now;
    for (const el of this.els) {
      if (Math.abs(el.currentTime - t) > 0.06) el.currentTime = t;
      if (el.paused) void el.play().catch(() => {});
    }
  }

  destroy() {
    for (const el of this.els) {
      el.pause();
      el.removeAttribute("src");
      el.load();
    }
    void this.ctx.close();
  }
}
