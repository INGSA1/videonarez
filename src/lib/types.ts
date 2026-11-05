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
