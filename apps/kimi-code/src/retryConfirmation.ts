export const RETRY_CONFIRMATION_PRESENTATION = "retry_confirmation";
export const RETRY_CONFIRMATION_ANSWER = "Retry";

export function isRetryConfirmationPayload(payload: unknown): boolean {
  return (
    typeof payload === "object" &&
    payload !== null &&
    "presentation" in payload &&
    payload.presentation === RETRY_CONFIRMATION_PRESENTATION
  );
}

export function retryConfirmationResponse(): {
  answers: Record<string, string>;
  method: "enter";
} {
  return {
    answers: { retry_confirmation: RETRY_CONFIRMATION_ANSWER },
    method: "enter",
  };
}
