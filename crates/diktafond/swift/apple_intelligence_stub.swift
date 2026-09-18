import Foundation

public typealias ResponsePointer = UnsafeMutablePointer<AppleIntelligenceResponse>

@_cdecl("apple_intelligence_availability")
public func appleIntelligenceAvailability() -> Int32 {
    -5
}

@_cdecl("apple_intelligence_polish")
public func appleIntelligencePolish(
    _ instructions: UnsafePointer<CChar>,
    _ transcript: UnsafePointer<CChar>,
    _ maxResponseTokens: Int32
) -> ResponsePointer {
    let response = ResponsePointer.allocate(capacity: 1)
    response.initialize(to: AppleIntelligenceResponse(
        response: nil,
        success: 0,
        error_message: strdup("Apple Intelligence is unavailable in this build")
    ))
    return response
}

@_cdecl("apple_intelligence_response_free")
public func appleIntelligenceResponseFree(_ response: ResponsePointer?) {
    guard let response else { return }
    free(response.pointee.response)
    free(response.pointee.error_message)
    response.deallocate()
}
