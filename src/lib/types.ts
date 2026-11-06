export interface AudioTrack {
  index: number;
  codec: string;
  channels: number;
  sampleRate: number;
  title?: string | null;
  language?: string | null;
}

export interface VideoMeta {
  path: string;
  fileName: string;
  sizeBytes: number;
  duration: number;
  startTime: number;
  formatName: string;
  bitRate?: number | null;
  hasVideo: boolean;
  width: number;
  height: number;
  displayWidth: number;
  displayHeight: number;
  rotation: number;
  fps: number;
  videoCodec: string;
  profile?: string | null;
  pixFmt: string;
  audioTracks: AudioTrack[];
  needsProxy: boolean;
}

export interface Marker {
  time: number;
  score: number;
}

export interface Waveform {
  path: string;
  track: number;
  peaks: number[];
  bucketSec: number;
  markers: Marker[];
}

export interface Thumb {
  index: number;
  time: number;
  data: string;
}

export type Vendor = "nvenc" | "amf" | "qsv" | "cpu";
export type Codec = "h264" | "hevc" | "av1";

export interface EncoderInfo {
  name: string;
  vendor: Vendor;
  codec: Codec;
  hardware: boolean;
}

export type Priority = "idle" | "belowNormal" | "normal";

export interface Settings {
  outputDir: string;
  priority: Priority;
  cpuThreads: number;
  encoders: EncoderInfo[];
  ui: any;
  capture: CaptureSettings;
}

export interface CaptureSettings {
  source: "monitor" | "window";
  monitor: number;
  windowExe: string;
  windowTitle: string;
  fps: number;
  height: number;
  codec: "h264" | "hevc";
  encoder: "auto" | Vendor;
  bitrateMbps: number;
  bufferSecs: number;
  systemAudio: boolean;
  appAudioOnly: boolean;
  mic: boolean;
  micDevice: string;
  audioOffsetMs: number;
  hotkeySave: string;
  hotkeyRecord: string;
  autoStart: boolean;
  sound: boolean;
  closeToTray: boolean;
}

export interface CaptureStatus {
  running: boolean;
  starting: boolean;
  pipeline: string;
  zeroCopy: boolean;
  encoder: string;
  source: string;
  bufferedSecs: number;
  bufferMb: number;
  maxSecs: number;
  fps: number;
  bitrateKbps: number;
  recordingSecs: number | null;
  uptimeSecs: number;
  error: string | null;
  audioErrors: string[];
  lastClip: string | null;
  ffmpegPid: number | null;
}

export interface MonitorInfo {
  index: number;
  name: string;
  width: number;
  height: number;
  x: number;
  y: number;
  primary: boolean;
}

export interface WindowInfo {
  hwnd: number;
  pid: number;
  title: string;
  exe: string;
  width: number;
  height: number;
}

export interface AudioDevice {
  id: string;
  name: string;
  default: boolean;
}

export interface CaptureSources {
  gpu: { name: string; vendorId: number; vendor: string } | null;
  monitors: MonitorInfo[];
  windows: WindowInfo[];
  mics: AudioDevice[];
}

export interface ClipSaved {
  path: string;
  duration: number;
  kind: "replay" | "recording";
}

export type TrimMode = "copy" | "smart" | "encode";
export type RateKind = "quality" | "bitrate" | "size";
export type AspectKey = "none" | "9:16" | "1:1" | "4:5" | "16:9" | "4:3";

export interface Segment {
  id: number;
  start: number;
  end: number;
  name: string;
}

export interface TrackOpt {
  enabled: boolean;
  volume: number;
}

export interface ExportOptions {
  preset: string;
  mode: TrimMode;
  container: "mp4" | "mkv";
  merge: boolean;
  encoder: "auto" | Vendor;
  codec: Codec;
  height: number | null;
  fps: number | null;
  aspect: AspectKey;
  cropX: number;
  cropY: number;
  rate: RateKind;
  quality: number;
  bitrateKbps: number;
  targetMb: number;
  audioBitrateKbps: number;
  mute: boolean;
  tracks: Record<number, TrackOpt>;
}

export interface ExportResult {
  files: string[];
  notes: string[];
  elapsedSec: number;
}

export interface ExportProgress {
  percent: number;
  stage: string;
  current: number;
  total: number;
}
