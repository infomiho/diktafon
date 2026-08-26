import Dispatch
import Foundation
import FoundationModels

public typealias ResponsePointer = UnsafeMutablePointer<AppleIntelligenceResponse>

private func copyCString(_ text: String) -> UnsafeMutablePointer<CChar>? {
    text.withCString { strdup($0) }
}

@_cdecl("apple_intelligence_availability")
public func appleIntelligenceAvailability() -> Int32 {
    guard #available(macOS 26.0, *) else { return -1 }
    switch SystemLanguageModel.default.availability {
    case .available:
        return 1
    case .unavailable(.deviceNotEligible):
        return -2
    case .unavailable(.appleIntelligenceNotEnabled):
        return -3
    case .unavailable(.modelNotReady):
        return -4
    case .unavailable:
        return -5
    }
}

@_cdecl("apple_intelligence_polish")
public func appleIntelligencePolish(
    _ instructions: UnsafePointer<CChar>,
    _ transcript: UnsafePointer<CChar>,
    _ maxResponseTokens: Int32
) -> ResponsePointer {
    let response = ResponsePointer.allocate(capacity: 1)
    response.initialize(to: AppleIntelligenceResponse(response: nil, success: 0, error_message: nil))

    guard #available(macOS 26.0, *) else {
        response.pointee.error_message = copyCString("Apple Intelligence requires macOS 26")
        return response
    }
    let model = SystemLanguageModel.default
    guard model.availability == .available else {
        response.pointee.error_message = copyCString("Apple Intelligence is not ready")
        return response
    }

    final class ResultBox: @unchecked Sendable {
        var output: String?
        var error: String?
    }
    let box = ResultBox()
    let semaphore = DispatchSemaphore(value: 0)
    let swiftInstructions = String(cString: instructions)
    let swiftTranscript = String(cString: transcript)
    let task = Task.detached(priority: .userInitiated) {
        defer { semaphore.signal() }
        do {
            let session = LanguageModelSession(model: model, instructions: swiftInstructions)
            let options = GenerationOptions(
                sampling: .greedy,
                maximumResponseTokens: max(1, Int(maxResponseTokens))
            )
            box.output = try await session.respond(to: swiftTranscript, options: options).content
        } catch {
            box.error = error.localizedDescription
        }
    }

    if semaphore.wait(timeout: .now() + .seconds(20)) == .timedOut {
        task.cancel()
        response.pointee.error_message = copyCString("Apple Intelligence timed out")
        return response
    }
    if let output = box.output, !output.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
        response.pointee.response = copyCString(output)
        response.pointee.success = 1
    } else {
        response.pointee.error_message = copyCString(box.error ?? "Apple Intelligence returned empty output")
    }
    return response
}

@_cdecl("apple_intelligence_response_free")
public func appleIntelligenceResponseFree(_ response: ResponsePointer?) {
    guard let response else { return }
    free(response.pointee.response)
    free(response.pointee.error_message)
    response.deallocate()
}
