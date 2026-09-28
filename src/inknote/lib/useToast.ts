import { useCallback, useEffect, useRef, useState } from "react";
import { formatError } from "./errors";

export type ToastKind = "info" | "success" | "error";

export type ToastState = {
  message: string;
  kind: ToastKind;
} | null;

const DISMISS_MS = 4200;
const TOAST_EVENT = "inknote:toast";

/** Let editor widgets use the same transient feedback as application actions. */
export function notifyToast(message: string, kind: ToastKind = "info") {
  window.dispatchEvent(new CustomEvent(TOAST_EVENT, { detail: { message, kind } }));
}

export function useToast() {
  const [toast, setToast] = useState<ToastState>(null);
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const dismiss = useCallback(() => {
    if (timerRef.current) clearTimeout(timerRef.current);
    timerRef.current = null;
    setToast(null);
  }, []);

  const show = useCallback(
    (message: string, kind: ToastKind = "info") => {
      setToast({ message, kind });
      if (timerRef.current) clearTimeout(timerRef.current);
      timerRef.current = setTimeout(dismiss, DISMISS_MS);
    },
    [dismiss],
  );

  const showSuccess = useCallback((message: string) => show(message, "success"), [show]);
  useEffect(() => {
    const onToast = (event: Event) => {
      const { message, kind } = (event as CustomEvent<NonNullable<ToastState>>).detail;
      show(message, kind);
    };
    window.addEventListener(TOAST_EVENT, onToast);
    return () => window.removeEventListener(TOAST_EVENT, onToast);
  }, [show]);
  const showError = useCallback(
    (e: unknown) => show(formatError(e), "error"),
    [show],
  );

  useEffect(() => () => {
    if (timerRef.current) clearTimeout(timerRef.current);
  }, []);

  return {
    message: toast?.message ?? null,
    toastKind: toast?.kind ?? "info",
    show,
    showSuccess,
    showError,
    dismiss,
  };
}
