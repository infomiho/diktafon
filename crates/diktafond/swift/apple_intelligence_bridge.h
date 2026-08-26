#ifndef DIKTAFON_APPLE_INTELLIGENCE_BRIDGE_H
#define DIKTAFON_APPLE_INTELLIGENCE_BRIDGE_H

typedef struct {
    char *response;
    int success;
    char *error_message;
} AppleIntelligenceResponse;

int apple_intelligence_availability(void);
AppleIntelligenceResponse *apple_intelligence_polish(
    const char *instructions,
    const char *transcript,
    int max_response_tokens
);
void apple_intelligence_response_free(AppleIntelligenceResponse *response);

#endif
