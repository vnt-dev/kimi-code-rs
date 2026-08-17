import {
  type KeyboardEvent,
  type MouseEvent,
  type PointerEvent as ReactPointerEvent,
  type UIEvent as ReactUIEvent,
  type WheelEvent,
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
} from "react";
import {
  conversationTurnScrollTarget,
  isChatAtBottom,
  isUpwardChatScrollKey,
  resolveChatFollowState,
} from "../chatScroll";

interface UseChatScrollOptions {
  conversationId?: string;
  historyLoading?: boolean;
  hasVisibleMessages: boolean;
  outlineItems: readonly unknown[];
}

function nestedVerticalScroller(
  target: EventTarget | null,
  root: HTMLElement,
): HTMLElement | undefined {
  let element = target instanceof HTMLElement ? target : undefined;
  while (element && element !== root) {
    const overflowY = window.getComputedStyle(element).overflowY;
    if (
      element.scrollHeight > element.clientHeight + 1 &&
      (overflowY === "auto" ||
        overflowY === "scroll" ||
        overflowY === "overlay")
    ) {
      return element;
    }
    element = element.parentElement ?? undefined;
  }
  return undefined;
}

function nestedScrollerConsumesWheel(
  target: EventTarget | null,
  root: HTMLElement,
  deltaY: number,
): boolean {
  let element = target instanceof HTMLElement ? target : undefined;
  while (element && element !== root) {
    const style = window.getComputedStyle(element);
    const scrollable =
      element.scrollHeight > element.clientHeight + 1 &&
      (style.overflowY === "auto" ||
        style.overflowY === "scroll" ||
        style.overflowY === "overlay");
    if (scrollable) {
      if (
        style.overscrollBehaviorY === "contain" ||
        style.overscrollBehaviorY === "none"
      ) {
        return true;
      }
      if (deltaY < 0 && element.scrollTop > 1) return true;
      if (
        deltaY > 0 &&
        element.scrollTop + element.clientHeight < element.scrollHeight - 1
      ) {
        return true;
      }
    }
    element = element.parentElement ?? undefined;
  }
  return false;
}

export function useChatScroll({
  conversationId,
  historyLoading,
  hasVisibleMessages,
  outlineItems,
}: UseChatScrollOptions) {
  const [activeOutlineTurnId, setActiveOutlineTurnId] = useState<string>();
  const scrollRef = useRef<HTMLDivElement>(null);
  const messageStackRef = useRef<HTMLDivElement>(null);
  const followLatestMessageRef = useRef(true);
  const lastChatScrollTopRef = useRef(0);
  const chatScrollFrameRef = useRef<number | undefined>(undefined);
  const chatScrollUpIntentRef = useRef(false);
  const chatScrollIntentFrameRef = useRef<number | undefined>(undefined);
  const chatDisclosureTimerRef = useRef<number | undefined>(undefined);
  const chatPointerScrollingRef = useRef(false);
  const chatPointerStartRef = useRef<
    | {
        pointerId: number;
        clientX: number;
        clientY: number;
      }
    | undefined
  >(undefined);
  const outlineScrollFrameRef = useRef<number | undefined>(undefined);

  const updateActiveOutlineTurn = useCallback((): void => {
    const scroll = scrollRef.current;
    if (!scroll) return;
    const anchors = Array.from(
      scroll.querySelectorAll<HTMLElement>("[data-conversation-turn-id]"),
    );
    if (anchors.length === 0) {
      setActiveOutlineTurnId(undefined);
      return;
    }

    const distanceFromBottom =
      scroll.scrollHeight - scroll.scrollTop - scroll.clientHeight;
    let nextId = anchors.at(-1)?.dataset.conversationTurnId;
    if (!isChatAtBottom(distanceFromBottom)) {
      const scrollRect = scroll.getBoundingClientRect();
      const viewportMiddle = scrollRect.top + scrollRect.height / 2;
      nextId = anchors[0]?.dataset.conversationTurnId;
      for (const anchor of anchors) {
        if (anchor.getBoundingClientRect().top > viewportMiddle) break;
        nextId = anchor.dataset.conversationTurnId;
      }
    }

    setActiveOutlineTurnId((current) =>
      current === nextId ? current : nextId,
    );
  }, []);

  const scheduleActiveOutlineTurnUpdate = useCallback((): void => {
    if (outlineScrollFrameRef.current !== undefined) return;
    outlineScrollFrameRef.current = window.requestAnimationFrame(() => {
      outlineScrollFrameRef.current = undefined;
      updateActiveOutlineTurn();
    });
  }, [updateActiveOutlineTurn]);

  useLayoutEffect(() => {
    if (chatDisclosureTimerRef.current !== undefined) {
      window.clearTimeout(chatDisclosureTimerRef.current);
      chatDisclosureTimerRef.current = undefined;
    }
    followLatestMessageRef.current = true;
    const scroll = scrollRef.current;
    if (scroll) {
      scroll.style.removeProperty("overflow-anchor");
      scroll.scrollTop = scroll.scrollHeight;
      lastChatScrollTopRef.current = scroll.scrollTop;
    }
  }, [conversationId, historyLoading]);

  useLayoutEffect(() => {
    const scroll = scrollRef.current;
    const content = messageStackRef.current;
    if (!scroll || !content || historyLoading) return;

    const scheduleScrollToLatest = (): void => {
      if (
        !followLatestMessageRef.current ||
        chatScrollFrameRef.current !== undefined
      ) {
        return;
      }
      chatScrollFrameRef.current = window.requestAnimationFrame(() => {
        chatScrollFrameRef.current = undefined;
        if (!followLatestMessageRef.current) return;
        scroll.scrollTop = scroll.scrollHeight;
        lastChatScrollTopRef.current = scroll.scrollTop;
      });
    };

    const observer = new ResizeObserver(scheduleScrollToLatest);
    observer.observe(content);
    scheduleScrollToLatest();
    return () => {
      observer.disconnect();
      if (chatScrollFrameRef.current !== undefined) {
        window.cancelAnimationFrame(chatScrollFrameRef.current);
        chatScrollFrameRef.current = undefined;
      }
    };
  }, [conversationId, historyLoading, hasVisibleMessages]);

  useLayoutEffect(() => {
    updateActiveOutlineTurn();
  }, [conversationId, outlineItems, updateActiveOutlineTurn]);

  useEffect(
    () => () => {
      if (outlineScrollFrameRef.current !== undefined) {
        window.cancelAnimationFrame(outlineScrollFrameRef.current);
      }
      if (chatScrollIntentFrameRef.current !== undefined) {
        window.cancelAnimationFrame(chatScrollIntentFrameRef.current);
      }
      if (chatDisclosureTimerRef.current !== undefined) {
        window.clearTimeout(chatDisclosureTimerRef.current);
      }
      scrollRef.current?.style.removeProperty("overflow-anchor");
    },
    [],
  );

  const markChatScrollUpIntent = useCallback((): void => {
    chatScrollUpIntentRef.current = true;
    if (chatScrollIntentFrameRef.current !== undefined) {
      window.cancelAnimationFrame(chatScrollIntentFrameRef.current);
    }
    chatScrollIntentFrameRef.current = window.requestAnimationFrame(() => {
      chatScrollIntentFrameRef.current = undefined;
      chatScrollUpIntentRef.current = false;
    });
  }, []);

  useEffect(() => {
    const stopPointerScrolling = (): void => {
      chatPointerScrollingRef.current = false;
      chatPointerStartRef.current = undefined;
    };
    const detectPointerScrolling = (event: PointerEvent): void => {
      const start = chatPointerStartRef.current;
      if (!start || start.pointerId !== event.pointerId) return;
      if (
        Math.abs(event.clientX - start.clientX) > 2 ||
        Math.abs(event.clientY - start.clientY) > 2
      ) {
        chatPointerScrollingRef.current = true;
        if (event.pointerType === "touch" && event.clientY > start.clientY) {
          markChatScrollUpIntent();
        }
        start.clientX = event.clientX;
        start.clientY = event.clientY;
      }
    };
    window.addEventListener("pointermove", detectPointerScrolling);
    window.addEventListener("pointerup", stopPointerScrolling);
    window.addEventListener("pointercancel", stopPointerScrolling);
    window.addEventListener("blur", stopPointerScrolling);
    return () => {
      window.removeEventListener("pointermove", detectPointerScrolling);
      window.removeEventListener("pointerup", stopPointerScrolling);
      window.removeEventListener("pointercancel", stopPointerScrolling);
      window.removeEventListener("blur", stopPointerScrolling);
    };
  }, [markChatScrollUpIntent]);

  const handleChatScroll = (event: ReactUIEvent<HTMLDivElement>): void => {
    if (event.target !== event.currentTarget) return;
    const scroll = event.currentTarget;
    scheduleActiveOutlineTurnUpdate();
    const distanceFromBottom =
      scroll.scrollHeight - scroll.scrollTop - scroll.clientHeight;
    const scrollingUp = scroll.scrollTop < lastChatScrollTopRef.current - 1;
    const userScrollingUp =
      chatScrollUpIntentRef.current || chatPointerScrollingRef.current;
    lastChatScrollTopRef.current = scroll.scrollTop;
    followLatestMessageRef.current = resolveChatFollowState({
      currentlyFollowing: followLatestMessageRef.current,
      distanceFromBottom,
      scrollingUp,
      userScrollingUp,
    });
  };

  const handleChatDisclosureClick = (
    event: MouseEvent<HTMLDivElement>,
  ): void => {
    const target = event.target;
    if (!(target instanceof Element)) return;
    const disclosure = target.closest("button[aria-expanded]");
    if (!disclosure || !event.currentTarget.contains(disclosure)) return;

    const scroll = event.currentTarget;
    followLatestMessageRef.current = false;
    scroll.style.overflowAnchor = "none";
    lastChatScrollTopRef.current = scroll.scrollTop;
    if (chatDisclosureTimerRef.current !== undefined) {
      window.clearTimeout(chatDisclosureTimerRef.current);
    }
    chatDisclosureTimerRef.current = window.setTimeout(() => {
      chatDisclosureTimerRef.current = undefined;
      scroll.style.removeProperty("overflow-anchor");
      lastChatScrollTopRef.current = scroll.scrollTop;
      const distanceFromBottom =
        scroll.scrollHeight - scroll.scrollTop - scroll.clientHeight;
      followLatestMessageRef.current = isChatAtBottom(distanceFromBottom);
    }, 240);
  };

  const handleChatWheel = (event: WheelEvent<HTMLDivElement>): void => {
    if (
      event.deltaY < 0 &&
      !nestedScrollerConsumesWheel(
        event.target,
        event.currentTarget,
        event.deltaY,
      )
    ) {
      markChatScrollUpIntent();
    }
  };

  const handleChatPointerDown = (
    event: ReactPointerEvent<HTMLDivElement>,
  ): void => {
    if (!event.isPrimary || event.button !== 0) return;
    chatPointerScrollingRef.current = false;
    chatPointerStartRef.current = nestedVerticalScroller(
      event.target,
      event.currentTarget,
    )
      ? undefined
      : {
          pointerId: event.pointerId,
          clientX: event.clientX,
          clientY: event.clientY,
        };
  };

  const handleChatKeyDown = (event: KeyboardEvent<HTMLDivElement>): void => {
    if (!isUpwardChatScrollKey(event.key, event.shiftKey)) return;
    const target = event.target;
    if (
      target instanceof HTMLElement &&
      (target.isContentEditable ||
        target.matches("input, textarea, select") ||
        (event.key === " " && target.matches("button")) ||
        nestedVerticalScroller(target, event.currentTarget))
    ) {
      return;
    }
    markChatScrollUpIntent();
  };

  const scrollToConversationTurn = (turnId: string): void => {
    const scroll = scrollRef.current;
    if (!scroll) return;
    const target = Array.from(
      scroll.querySelectorAll<HTMLElement>("[data-conversation-turn-id]"),
    ).find((anchor) => anchor.dataset.conversationTurnId === turnId);
    if (!target) return;
    followLatestMessageRef.current = false;
    setActiveOutlineTurnId(turnId);
    conversationTurnScrollTarget(target).scrollIntoView({
      behavior: "smooth",
      block: "start",
    });
  };

  return {
    activeOutlineTurnId,
    followLatestMessageRef,
    scrollRef,
    messageStackRef,
    handleChatScroll,
    handleChatDisclosureClick,
    handleChatWheel,
    handleChatPointerDown,
    handleChatKeyDown,
    scrollToConversationTurn,
  };
}
