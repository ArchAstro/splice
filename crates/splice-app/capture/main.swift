// splice-capture: the sources Splice records from (a screen or window and
// the sound the Mac plays, a camera, a microphone), kept live and shown
// small to Splice, and written on request each to its own file, all on one
// clock, so that the files start and end together.
//
// Splice runs it and talks to it:
//
//   splice-capture devices        cameras and microphones, as JSON
//   splice-capture run            the sources in the JSON line on stdin
//
// While running, it reads commands as JSON lines on stdin (record, pause,
// resume, stop, quit) and writes messages on stdout, each a kind byte and a
// little-endian u32 length before its body: "J" an event as JSON, "F" a
// preview frame (u8 source, u32 width, u32 height, then BGRA pixels).
// Closing stdin quits, finishing any file being written.

import AVFoundation
import CoreImage
import CoreMedia
import Foundation
import ScreenCaptureKit

// MARK: - Talking to Splice

let out = FileHandle.standardOutput
let outLock = NSLock()

func send(_ kind: UInt8, _ body: Data) {
    var message = Data([kind])
    var length = UInt32(body.count).littleEndian
    message.append(Data(bytes: &length, count: 4))
    message.append(body)
    outLock.lock()
    out.write(message)
    outLock.unlock()
}

func event(_ name: String, _ fields: [String: Any] = [:]) {
    var all = fields
    all["event"] = name
    if let data = try? JSONSerialization.data(withJSONObject: all, options: [.sortedKeys]) {
        send(UInt8(ascii: "J"), data)
    }
}

func fail(_ message: String) -> Never {
    event("error", ["message": message])
    exit(1)
}

// MARK: - Devices

func cameras() -> [AVCaptureDevice] {
    var types: [AVCaptureDevice.DeviceType] = [.builtInWideAngleCamera, .deskViewCamera]
    if #available(macOS 14.0, *) {
        types += [.external, .continuityCamera]
    } else {
        types.append(.externalUnknown)
    }
    return AVCaptureDevice.DiscoverySession(deviceTypes: types, mediaType: .video, position: .unspecified).devices
}

func microphones() -> [AVCaptureDevice] {
    var types: [AVCaptureDevice.DeviceType] = []
    if #available(macOS 14.0, *) {
        types += [.microphone, .external]
    } else {
        types += [.builtInMicrophone, .externalUnknown]
    }
    return AVCaptureDevice.DiscoverySession(deviceTypes: types, mediaType: .audio, position: .unspecified).devices
}

func listDevices() {
    func describe(_ d: AVCaptureDevice, _ fallback: AVCaptureDevice?) -> [String: Any] {
        ["id": d.uniqueID, "name": d.localizedName, "default": d.uniqueID == fallback?.uniqueID]
    }
    let camera = AVCaptureDevice.default(for: .video)
    let microphone = AVCaptureDevice.default(for: .audio)
    let body: [String: Any] = [
        "cameras": cameras().map { describe($0, camera) },
        "microphones": microphones().map { describe($0, microphone) },
        "camera_access": access(.video),
        "microphone_access": access(.audio),
        "screen_access": CGPreflightScreenCaptureAccess(),
    ]
    let data = try! JSONSerialization.data(withJSONObject: body, options: [.sortedKeys])
    FileHandle.standardOutput.write(data)
    FileHandle.standardOutput.write("\n".data(using: .utf8)!)
}

func access(_ media: AVMediaType) -> String {
    switch AVCaptureDevice.authorizationStatus(for: media) {
    case .authorized: return "granted"
    case .denied: return "denied"
    case .restricted: return "restricted"
    case .notDetermined: return "unasked"
    @unknown default: return "unknown"
    }
}

/// Asks for camera or microphone access if it has not been asked, and waits.
func allow(_ media: AVMediaType) -> Bool {
    switch AVCaptureDevice.authorizationStatus(for: media) {
    case .authorized: return true
    case .notDetermined:
        let done = DispatchSemaphore(value: 0)
        var granted = false
        AVCaptureDevice.requestAccess(for: media) { ok in
            granted = ok
            done.signal()
        }
        done.wait()
        return granted
    default: return false
    }
}

// MARK: - What to capture

struct Screen: Decodable {
    /// A display, by its CoreGraphics id.
    var display: UInt32?
    var window: UInt32?
    /// Part of the display, in points from its top left: x, y, width, height.
    var region: [Double]?
}

struct Config: Decodable {
    var screen: Screen?
    var systemAudio: Bool = false
    var camera: String?
    var microphone: String?
    var codec: String = "h264"
    /// 0 to 1: the encoder's constant quality.
    var quality: Double = 0.65
    var fps: Int = 30
    /// The screen file is made no taller than this (keeping its shape).
    var maxHeight: Int?
    var cursor: Bool = true
    /// A process whose windows stay out of the shot (Splice).
    var exclude: Int32?
    /// The longest side of preview frames; 0 sends none.
    var preview: Int = 480
    var previewFps: Int = 12

    enum CodingKeys: String, CodingKey {
        case screen, camera, microphone, codec, quality, fps, cursor, exclude, preview
        case systemAudio = "system_audio"
        case maxHeight = "max_height"
        case previewFps = "preview_fps"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        screen = try c.decodeIfPresent(Screen.self, forKey: .screen)
        systemAudio = try c.decodeIfPresent(Bool.self, forKey: .systemAudio) ?? false
        camera = try c.decodeIfPresent(String.self, forKey: .camera)
        microphone = try c.decodeIfPresent(String.self, forKey: .microphone)
        codec = try c.decodeIfPresent(String.self, forKey: .codec) ?? "h264"
        quality = try c.decodeIfPresent(Double.self, forKey: .quality) ?? 0.65
        fps = try c.decodeIfPresent(Int.self, forKey: .fps) ?? 30
        maxHeight = try c.decodeIfPresent(Int.self, forKey: .maxHeight)
        cursor = try c.decodeIfPresent(Bool.self, forKey: .cursor) ?? true
        exclude = try c.decodeIfPresent(Int32.self, forKey: .exclude)
        preview = try c.decodeIfPresent(Int.self, forKey: .preview) ?? 480
        previewFps = try c.decodeIfPresent(Int.self, forKey: .previewFps) ?? 12
    }
}

/// The sources, in the order their files and preview frames are numbered.
enum Feed: Int, CaseIterable {
    case screen = 0, camera = 1, microphone = 2, system = 3

    var name: String {
        switch self {
        case .screen: return "screen"
        case .camera: return "camera"
        case .microphone: return "microphone"
        case .system: return "system"
        }
    }

    var isVideo: Bool { self == .screen || self == .camera }
    var file: String { name + (isVideo ? ".mp4" : ".m4a") }
}

// MARK: - Time

let hostClock = CMClockGetHostTimeClock()

func hostNow() -> CMTime { CMClockGetTime(hostClock) }

/// The same buffer, every timing shifted by `delta`.
func shifted(_ buffer: CMSampleBuffer, by delta: CMTime) -> CMSampleBuffer? {
    var count: CMItemCount = 0
    CMSampleBufferGetSampleTimingInfoArray(buffer, entryCount: 0, arrayToFill: nil, entriesNeededOut: &count)
    var timing = [CMSampleTimingInfo](repeating: CMSampleTimingInfo(), count: max(count, 1))
    CMSampleBufferGetSampleTimingInfoArray(buffer, entryCount: timing.count, arrayToFill: &timing, entriesNeededOut: &count)
    for i in 0..<timing.count {
        timing[i].presentationTimeStamp = CMTimeAdd(timing[i].presentationTimeStamp, delta)
        if timing[i].decodeTimeStamp.isValid {
            timing[i].decodeTimeStamp = CMTimeAdd(timing[i].decodeTimeStamp, delta)
        }
    }
    var result: CMSampleBuffer?
    CMSampleBufferCreateCopyWithNewTiming(
        allocator: nil, sampleBuffer: buffer, sampleTimingEntryCount: timing.count,
        sampleTimingArray: &timing, sampleBufferOut: &result)
    return result
}

/// The same video frame at `time`.
func at(_ buffer: CMSampleBuffer, _ time: CMTime) -> CMSampleBuffer? {
    shifted(buffer, by: CMTimeSubtract(time, CMSampleBufferGetPresentationTimeStamp(buffer)))
}

// MARK: - Files

/// One source's file.
final class Sink {
    let feed: Feed
    let writer: AVAssetWriter
    let input: AVAssetWriterInput
    var appended = 0
    var dropped = 0

    init(feed: Feed, url: URL, sample: CMSampleBuffer, config: Config) throws {
        self.feed = feed
        try? FileManager.default.removeItem(at: url)
        writer = try AVAssetWriter(outputURL: url, fileType: feed.isVideo ? .mp4 : .m4a)
        // Written in fragments, so a crash leaves a file that plays.
        writer.movieFragmentInterval = CMTime(seconds: 1, preferredTimescale: 600)
        let format = CMSampleBufferGetFormatDescription(sample)
        if feed.isVideo {
            let size = format.map { CMVideoFormatDescriptionGetDimensions($0) } ?? CMVideoDimensions(width: 1920, height: 1080)
            let hevc = config.codec == "hevc"
            let quality = max(0.05, min(1.0, config.quality))
            var compression: [String: Any] = [
                AVVideoExpectedSourceFrameRateKey: config.fps,
                AVVideoMaxKeyFrameIntervalDurationKey: 2.0,
                AVVideoAllowFrameReorderingKey: false,
            ]
            if hevc {
                compression[AVVideoQualityKey] = quality
            } else {
                // H.264 here takes a bitrate, not a quality: bits for each
                // pixel of each frame, more as the quality rises.
                let perPixel = 0.04 + quality * 0.16
                compression[AVVideoAverageBitRateKey] = Int(Double(size.width) * Double(size.height) * Double(max(1, config.fps)) * perPixel)
            }
            input = AVAssetWriterInput(mediaType: .video, outputSettings: [
                AVVideoCodecKey: hevc ? AVVideoCodecType.hevc : AVVideoCodecType.h264,
                AVVideoWidthKey: Int(size.width),
                AVVideoHeightKey: Int(size.height),
                AVVideoCompressionPropertiesKey: compression,
            ])
        } else {
            let asbd = format.flatMap { CMAudioFormatDescriptionGetStreamBasicDescription($0)?.pointee }
            let channels = min(Int(asbd?.mChannelsPerFrame ?? 2), 2)
            input = AVAssetWriterInput(mediaType: .audio, outputSettings: [
                AVFormatIDKey: kAudioFormatMPEG4AAC,
                AVSampleRateKey: asbd?.mSampleRate ?? 48000,
                AVNumberOfChannelsKey: channels,
                AVEncoderBitRateKey: channels == 1 ? 128_000 : 256_000,
            ])
        }
        input.expectsMediaDataInRealTime = true
        guard writer.canAdd(input) else { throw NSError(domain: "splice-capture", code: 1) }
        writer.add(input)
        guard writer.startWriting() else {
            throw writer.error ?? NSError(domain: "splice-capture", code: 2)
        }
        writer.startSession(atSourceTime: .zero)
    }

    func append(_ buffer: CMSampleBuffer) {
        guard writer.status == .writing else { return }
        if input.isReadyForMoreMediaData, input.append(buffer) {
            appended += 1
        } else {
            dropped += 1
        }
    }

    func finish(at end: CMTime, _ done: @escaping () -> Void) {
        guard writer.status == .writing else { return done() }
        input.markAsFinished()
        writer.endSession(atSourceTime: end)
        writer.finishWriting(completionHandler: done)
    }
}

// MARK: - The capture

final class Capture: NSObject, SCStreamOutput, SCStreamDelegate,
    AVCaptureVideoDataOutputSampleBufferDelegate, AVCaptureAudioDataOutputSampleBufferDelegate
{
    let config: Config
    /// Everything that touches a buffer happens here, one at a time.
    let queue = DispatchQueue(label: "splice-capture", qos: .userInteractive)

    var streams: [SCStream] = []
    var cameraSession: AVCaptureSession?
    var micSession: AVCaptureSession?
    var cameraOutput: AVCaptureVideoDataOutput?
    var micOutput: AVCaptureAudioDataOutput?

    /// The latest frame from each video source.
    var latest: [Feed: CMSampleBuffer] = [:]
    var live: Set<Feed> = []
    var expected: Set<Feed> = []

    // While recording: the host time the files start at, how long has been
    // paused, and since when.
    var dir: URL?
    var zero: CMTime?
    var pausedFor = CMTime.zero
    var pausedAt: CMTime?
    var sinks: [Feed: Sink] = [:]

    var lastPreview: [Feed: CMTime] = [:]
    let context = CIContext(options: [.cacheIntermediates: false])
    var peaks: [Feed: Float] = [:]
    var lastLevels = CMTime.zero

    init(config: Config) {
        self.config = config
    }

    // MARK: Starting

    func start() async throws {
        if config.screen != nil || config.systemAudio {
            if !CGPreflightScreenCaptureAccess() {
                CGRequestScreenCaptureAccess()
                throw Problem("Splice needs Screen Recording permission: turn it on in System Settings → Privacy & Security → Screen Recording, then reopen Splice")
            }
            try await startScreen()
        }
        if let id = config.camera {
            guard allow(.video) else {
                throw Problem("Splice needs Camera permission: turn it on in System Settings → Privacy & Security → Camera")
            }
            try startCamera(id)
        }
        if let id = config.microphone {
            guard allow(.audio) else {
                throw Problem("Splice needs Microphone permission: turn it on in System Settings → Privacy & Security → Microphone")
            }
            try startMicrophone(id)
        }
    }

    func startScreen() async throws {
        let content = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: true)
        let away = content.applications.filter { $0.processID == config.exclude }
        guard let shown = config.screen else {
            if config.systemAudio { try await startSound(content, away) }
            return
        }
        let filter: SCContentFilter
        let settings = SCStreamConfiguration()
        var size: CGSize
        var scale: CGFloat = 2
        // A window's filter hears only its own app: the Mac's sound then
        // comes from the whole display, apart.
        var sound = config.systemAudio
        if let id = shown.window {
            guard let window = content.windows.first(where: { $0.windowID == id }) else {
                throw Problem("that window is not on screen any more")
            }
            filter = SCContentFilter(desktopIndependentWindow: window)
            size = window.frame.size
            scale = backing(for: window.frame)
            sound = false
        } else {
            let wanted = shown.display ?? CGMainDisplayID()
            guard let display = content.displays.first(where: { $0.displayID == wanted }) ?? content.displays.first else {
                throw Problem("no display to record")
            }
            filter = SCContentFilter(display: display, excludingApplications: away, exceptingWindows: [])
            size = CGSize(width: display.width, height: display.height)
            scale = backing(for: CGDisplayBounds(display.displayID))
            if let r = shown.region, r.count == 4 {
                settings.sourceRect = CGRect(x: r[0], y: r[1], width: r[2], height: r[3])
                size = CGSize(width: r[2], height: r[3])
            }
        }
        if #available(macOS 14.0, *) {
            scale = CGFloat(filter.pointPixelScale)
        }
        var pixels = CGSize(width: size.width * scale, height: size.height * scale)
        if let most = config.maxHeight, most > 0, pixels.height > CGFloat(most) {
            pixels = CGSize(width: pixels.width * CGFloat(most) / pixels.height, height: CGFloat(most))
        }
        settings.width = max(2, Int(pixels.width) / 2 * 2)
        settings.height = max(2, Int(pixels.height) / 2 * 2)
        settings.minimumFrameInterval = CMTime(value: 1, timescale: CMTimeScale(max(1, config.fps)))
        settings.pixelFormat = kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange
        settings.colorMatrix = CGDisplayStream.yCbCrMatrix_ITU_R_709_2
        settings.showsCursor = config.cursor
        settings.queueDepth = 8
        if sound { hear(settings) }
        let stream = SCStream(filter: filter, configuration: settings, delegate: self)
        try stream.addStreamOutput(self, type: .screen, sampleHandlerQueue: queue)
        expected.insert(.screen)
        if sound {
            try stream.addStreamOutput(self, type: .audio, sampleHandlerQueue: queue)
            expected.insert(.system)
        }
        try await stream.startCapture()
        streams.append(stream)
        if config.systemAudio, !sound {
            try await startSound(content, away)
        }
    }

    /// The Mac's sound, from the main display, its picture kept tiny and unread.
    func startSound(_ content: SCShareableContent, _ away: [SCRunningApplication]) async throws {
        guard let display = content.displays.first(where: { $0.displayID == CGMainDisplayID() }) ?? content.displays.first else {
            throw Problem("no display to hear the Mac's sound from")
        }
        let filter = SCContentFilter(display: display, excludingApplications: away, exceptingWindows: [])
        let settings = SCStreamConfiguration()
        settings.width = 64
        settings.height = 64
        settings.minimumFrameInterval = CMTime(value: 1, timescale: 1)
        hear(settings)
        let stream = SCStream(filter: filter, configuration: settings, delegate: self)
        try stream.addStreamOutput(self, type: .audio, sampleHandlerQueue: queue)
        expected.insert(.system)
        try await stream.startCapture()
        streams.append(stream)
    }

    func hear(_ settings: SCStreamConfiguration) {
        settings.capturesAudio = true
        settings.excludesCurrentProcessAudio = true
        settings.sampleRate = 48000
        settings.channelCount = 2
    }

    func backing(for rect: CGRect) -> CGFloat {
        NSScreenScale(containing: rect)
    }

    func startCamera(_ id: String) throws {
        guard let device = AVCaptureDevice(uniqueID: id) ?? cameras().first(where: { $0.localizedName == id }) else {
            throw Problem("no camera \(id)")
        }
        let session = AVCaptureSession()
        session.beginConfiguration()
        let input = try AVCaptureDeviceInput(device: device)
        guard session.canAddInput(input) else { throw Problem("the camera is busy") }
        session.addInput(input)
        // The largest picture no bigger than 1080p at the frame rate wanted.
        let fps = Double(config.fps)
        let fitting = device.formats.filter { f in
            let d = CMVideoFormatDescriptionGetDimensions(f.formatDescription)
            return d.height <= 1080 && d.width <= 1920
                && f.videoSupportedFrameRateRanges.contains { $0.maxFrameRate >= fps - 0.5 }
        }
        let best = fitting.max { a, b in
            let (x, y) = (CMVideoFormatDescriptionGetDimensions(a.formatDescription), CMVideoFormatDescriptionGetDimensions(b.formatDescription))
            return Int(x.width) * Int(x.height) < Int(y.width) * Int(y.height)
        }
        if let best, (try? device.lockForConfiguration()) != nil {
            device.activeFormat = best
            if let range = best.videoSupportedFrameRateRanges.first(where: { $0.maxFrameRate >= fps - 0.5 }) {
                let frame = CMTime(value: 1000, timescale: CMTimeScale(min(fps, range.maxFrameRate) * 1000))
                device.activeVideoMinFrameDuration = frame
                device.activeVideoMaxFrameDuration = frame
            }
            device.unlockForConfiguration()
        }
        let output = AVCaptureVideoDataOutput()
        output.videoSettings = [kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange]
        output.alwaysDiscardsLateVideoFrames = true
        output.setSampleBufferDelegate(self, queue: queue)
        guard session.canAddOutput(output) else { throw Problem("the camera cannot be read") }
        session.addOutput(output)
        session.commitConfiguration()
        session.startRunning()
        cameraSession = session
        cameraOutput = output
        expected.insert(.camera)
    }

    func startMicrophone(_ id: String) throws {
        guard let device = AVCaptureDevice(uniqueID: id) ?? microphones().first(where: { $0.localizedName == id }) else {
            throw Problem("no microphone \(id)")
        }
        let session = AVCaptureSession()
        session.beginConfiguration()
        let input = try AVCaptureDeviceInput(device: device)
        guard session.canAddInput(input) else { throw Problem("the microphone is busy") }
        session.addInput(input)
        let output = AVCaptureAudioDataOutput()
        // One channel of voice, at 48 kHz.
        output.audioSettings = [
            AVFormatIDKey: kAudioFormatLinearPCM,
            AVSampleRateKey: 48000,
            AVNumberOfChannelsKey: 1,
            AVLinearPCMBitDepthKey: 32,
            AVLinearPCMIsFloatKey: true,
            AVLinearPCMIsNonInterleaved: false,
        ]
        output.setSampleBufferDelegate(self, queue: queue)
        guard session.canAddOutput(output) else { throw Problem("the microphone cannot be read") }
        session.addOutput(output)
        session.commitConfiguration()
        session.startRunning()
        micSession = session
        micOutput = output
        expected.insert(.microphone)
    }

    // MARK: Buffers

    func stream(_ stream: SCStream, didOutputSampleBuffer buffer: CMSampleBuffer, of type: SCStreamOutputType) {
        guard buffer.isValid else { return }
        switch type {
        case .screen:
            // Frames that are not complete say only that nothing changed.
            guard let attachments = CMSampleBufferGetSampleAttachmentsArray(buffer, createIfNecessary: false) as? [[SCStreamFrameInfo: Any]],
                let raw = attachments.first?[.status] as? Int, SCFrameStatus(rawValue: raw) == .complete
            else { return }
            take(buffer, .screen, CMSampleBufferGetPresentationTimeStamp(buffer))
        case .audio:
            take(buffer, .system, CMSampleBufferGetPresentationTimeStamp(buffer))
        default:
            break
        }
    }

    func stream(_ stream: SCStream, didStopWithError error: Error) {
        queue.async {
            if self.zero != nil { self.finish(reason: "the screen stopped being recorded: \(error.localizedDescription)") }
            else { fail("the screen stopped being recorded: \(error.localizedDescription)") }
        }
    }

    func captureOutput(_ output: AVCaptureOutput, didOutput buffer: CMSampleBuffer, from connection: AVCaptureConnection) {
        let feed: Feed = output === cameraOutput ? .camera : .microphone
        let session = feed == .camera ? cameraSession : micSession
        // On the host clock, as the screen is.
        let time = CMSampleBufferGetPresentationTimeStamp(buffer)
        let clock = session?.synchronizationClock ?? hostClock
        take(buffer, feed, CMSyncConvertTime(time, from: clock, to: hostClock), clock: clock)
    }

    /// A buffer from `feed`, presented at `time` on the host clock.
    func take(_ buffer: CMSampleBuffer, _ feed: Feed, _ time: CMTime, clock: CMClock? = nil) {
        var buffer = buffer
        if clock != nil, clock !== hostClock {
            let delta = CMTimeSubtract(time, CMSampleBufferGetPresentationTimeStamp(buffer))
            guard let moved = shifted(buffer, by: delta) else { return }
            buffer = moved
        }
        if !live.contains(feed) {
            live.insert(feed)
            if live == expected { event("live", ["sources": expected.map(\.name).sorted()]) }
        }
        if feed.isVideo {
            latest[feed] = buffer
            preview(buffer, feed, time)
        } else {
            level(buffer, feed, time)
        }
        guard let zero, pausedAt == nil, let dir else { return }
        let local = CMTimeSubtract(CMTimeSubtract(time, zero), pausedFor)
        if local < .zero { return }
        guard let moved = shifted(buffer, by: CMTimeSubtract(local, time)) else { return }
        sink(feed, dir, moved)?.append(moved)
    }

    func sink(_ feed: Feed, _ dir: URL, _ sample: CMSampleBuffer) -> Sink? {
        if let s = sinks[feed] { return s }
        do {
            let s = try Sink(feed: feed, url: dir.appendingPathComponent(feed.file), sample: sample, config: config)
            sinks[feed] = s
            return s
        } catch {
            event("warning", ["message": "could not write the \(feed.name): \(error.localizedDescription)"])
            return nil
        }
    }

    // MARK: Recording

    func record(into dir: URL) {
        guard zero == nil else { return event("warning", ["message": "already recording"]) }
        do {
            try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        } catch {
            return event("warning", ["message": "could not make \(dir.path): \(error.localizedDescription)"])
        }
        let now = hostNow()
        self.dir = dir
        zero = now
        pausedFor = .zero
        pausedAt = nil
        // A picture that is not changing sends no frames: the files begin
        // with what each source shows now.
        for (feed, frame) in latest {
            if let first = at(frame, .zero) { sink(feed, dir, first)?.append(first) }
        }
        event("recording", ["dir": dir.path, "wall": wall(at: now)])
    }

    func pause() {
        guard zero != nil, pausedAt == nil else { return }
        let now = hostNow()
        pausedAt = now
        event("paused", ["seconds": seconds(), "wall": wall(at: now)])
    }

    func resume() {
        guard let since = pausedAt, let zero, let dir else { return }
        let now = hostNow()
        pausedFor = CMTimeAdd(pausedFor, CMTimeSubtract(now, since))
        pausedAt = nil
        let local = CMTimeSubtract(CMTimeSubtract(now, zero), pausedFor)
        for (feed, frame) in latest {
            if let again = at(frame, local) { sink(feed, dir, again)?.append(again) }
        }
        event("resumed", ["seconds": seconds(), "wall": wall(at: now)])
    }

    /// How long has been recorded, not counting pauses.
    func seconds() -> Double {
        guard let zero else { return 0 }
        let end = pausedAt ?? hostNow()
        return CMTimeSubtract(CMTimeSubtract(end, zero), pausedFor).seconds
    }

    /// Finishes the files. Each ends at the same moment.
    func finish(reason: String? = nil, then: (() -> Void)? = nil) {
        guard zero != nil, let dir else {
            then?()
            return
        }
        let length = CMTime(seconds: seconds(), preferredTimescale: 600)
        let group = DispatchGroup()
        var files: [String: Any] = [:]
        var dropped: [String: Int] = [:]
        for (feed, s) in sinks {
            // The picture holds to the end, however long since it changed.
            if feed.isVideo, let frame = latest[feed], let last = at(frame, length) { s.append(last) }
            group.enter()
            files[feed.name] = dir.appendingPathComponent(feed.file).path
            if s.dropped > 0 { dropped[feed.name] = s.dropped }
            s.finish(at: length) { group.leave() }
        }
        group.wait()
        let failed = sinks.filter { $0.value.writer.status != .completed }
            .map { "\($0.key.name): \($0.value.writer.error?.localizedDescription ?? "not finished")" }
        sinks = [:]
        zero = nil
        self.dir = nil
        pausedAt = nil
        var fields: [String: Any] = ["dir": dir.path, "files": files, "seconds": length.seconds]
        if !dropped.isEmpty { fields["dropped"] = dropped }
        if !failed.isEmpty { fields["failed"] = failed }
        if let reason { fields["reason"] = reason }
        event("stopped", fields)
        then?()
    }

    func shutdown() {
        let done = DispatchSemaphore(value: 0)
        queue.async {
            self.finish { done.signal() }
        }
        done.wait()
        cameraSession?.stopRunning()
        micSession?.stopRunning()
        for stream in streams {
            let stopped = DispatchSemaphore(value: 0)
            stream.stopCapture { _ in stopped.signal() }
            _ = stopped.wait(timeout: .now() + 2)
        }
    }

    // MARK: Preview and levels

    func preview(_ buffer: CMSampleBuffer, _ feed: Feed, _ time: CMTime) {
        guard config.preview > 0, let pixels = CMSampleBufferGetImageBuffer(buffer) else { return }
        if let last = lastPreview[feed], CMTimeSubtract(time, last).seconds < 1.0 / Double(max(1, config.previewFps)) {
            return
        }
        lastPreview[feed] = time
        let image = CIImage(cvPixelBuffer: pixels)
        let (w, h) = (image.extent.width, image.extent.height)
        let k = min(1, CGFloat(config.preview) / max(w, h))
        let (pw, ph) = (max(2, Int(w * k)), max(2, Int(h * k)))
        let small = image.transformed(by: CGAffineTransform(scaleX: CGFloat(pw) / w, y: CGFloat(ph) / h))
        var body = Data(count: 9 + pw * ph * 4)
        body.withUnsafeMutableBytes { raw in
            let p = raw.baseAddress!
            p.storeBytes(of: UInt8(feed.rawValue), as: UInt8.self)
            p.advanced(by: 1).storeBytes(of: UInt32(pw).littleEndian, toByteOffset: 0, as: UInt32.self)
            p.advanced(by: 5).storeBytes(of: UInt32(ph).littleEndian, toByteOffset: 0, as: UInt32.self)
            context.render(
                small, toBitmap: p.advanced(by: 9), rowBytes: pw * 4,
                bounds: CGRect(x: 0, y: 0, width: pw, height: ph), format: .BGRA8,
                colorSpace: CGColorSpace(name: CGColorSpace.sRGB))
        }
        send(UInt8(ascii: "F"), body)
    }

    /// The loudest sample of each sound, sent ten times a second.
    func level(_ buffer: CMSampleBuffer, _ feed: Feed, _ time: CMTime) {
        var peak: Float = peaks[feed] ?? 0
        // Float samples, all channels one after another or interleaved:
        // the loudest of them is the same either way.
        if let format = CMSampleBufferGetFormatDescription(buffer),
            let asbd = CMAudioFormatDescriptionGetStreamBasicDescription(format)?.pointee,
            asbd.mFormatFlags & kAudioFormatFlagIsFloat != 0, asbd.mBitsPerChannel == 32,
            let block = CMSampleBufferGetDataBuffer(buffer)
        {
            var length = 0
            var data: UnsafeMutablePointer<CChar>?
            if CMBlockBufferGetDataPointer(block, atOffset: 0, lengthAtOffsetOut: nil, totalLengthOut: &length, dataPointerOut: &data) == kCMBlockBufferNoErr,
                let data, CMBlockBufferIsRangeContiguous(block, atOffset: 0, length: length)
            {
                let n = length / 4
                data.withMemoryRebound(to: Float.self, capacity: n) { samples in
                    for i in 0..<n { peak = max(peak, abs(samples[i])) }
                }
            }
        }
        peaks[feed] = peak
        if CMTimeSubtract(time, lastLevels).seconds >= 0.1 {
            lastLevels = time
            var fields: [String: Any] = [:]
            for (f, p) in peaks { fields[f.name] = Double(p) }
            if zero != nil { fields["seconds"] = seconds() }
            event("levels", fields)
            peaks = [:]
        }
    }
}

struct Problem: LocalizedError {
    let message: String
    init(_ message: String) { self.message = message }
    var errorDescription: String? { message }
}

/// The backing scale of the screen holding most of `rect` (CoreGraphics
/// coordinates, from the main display's top left).
func NSScreenScale(containing rect: CGRect) -> CGFloat {
    var ids = [CGDirectDisplayID](repeating: 0, count: 16)
    var count: UInt32 = 0
    CGGetDisplaysWithRect(rect, 16, &ids, &count)
    guard count > 0, let mode = CGDisplayCopyDisplayMode(ids[0]) else { return 2 }
    return CGFloat(mode.pixelWidth) / CGFloat(max(1, mode.width))
}

/// Seconds since 1970 at host time `t`.
func wall(at t: CMTime) -> Double {
    Date().timeIntervalSince1970 - CMTimeSubtract(hostNow(), t).seconds
}

// MARK: - Main

let arguments = CommandLine.arguments.dropFirst()
switch arguments.first {
case "devices":
    listDevices()
    exit(0)
case "run":
    break
default:
    FileHandle.standardError.write("usage: splice-capture devices | run (the sources as a JSON line on stdin)\n".data(using: .utf8)!)
    exit(2)
}

guard let line = readLine(), let data = line.data(using: .utf8) else { fail("no sources given") }
let config: Config
do {
    config = try JSONDecoder().decode(Config.self, from: data)
} catch {
    fail("could not read the sources: \(error)")
}

let capture = Capture(config: config)
signal(SIGPIPE, SIG_IGN)

Task {
    do {
        try await capture.start()
    } catch {
        fail(error.localizedDescription)
    }
    event("started", ["sources": capture.expected.map(\.name).sorted()])
    // A source that never sends anything is worth knowing about.
    capture.queue.asyncAfter(deadline: .now() + 4) {
        let silent = capture.expected.subtracting(capture.live)
        if !silent.isEmpty { event("warning", ["message": "nothing yet from: \(silent.map(\.name).sorted().joined(separator: ", "))"]) }
    }
}

for sig in [SIGINT, SIGTERM] {
    signal(sig, SIG_IGN)
    let source = DispatchSource.makeSignalSource(signal: sig, queue: .global())
    source.setEventHandler {
        capture.shutdown()
        exit(0)
    }
    source.resume()
    _ = Unmanaged.passRetained(source)
}

Thread.detachNewThread {
    while let line = readLine() {
        guard let data = line.data(using: .utf8),
            let command = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
            let name = command["cmd"] as? String
        else { continue }
        switch name {
        case "record":
            guard let path = command["dir"] as? String else { continue }
            capture.queue.async { capture.record(into: URL(fileURLWithPath: path)) }
        case "pause":
            capture.queue.async { capture.pause() }
        case "resume":
            capture.queue.async { capture.resume() }
        case "stop":
            capture.queue.async { capture.finish() }
        case "quit":
            capture.shutdown()
            exit(0)
        default:
            event("warning", ["message": "unknown command \(name)"])
        }
    }
    // Splice went away: finish what is being written.
    capture.shutdown()
    exit(0)
}

dispatchMain()
