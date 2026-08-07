import { useState } from "react";
import { CircleUserRound } from "lucide-react";

import type { AccountProfile } from "../types";

export function AccountAvatar({
  profile,
  size,
}: {
  profile?: AccountProfile;
  size: number;
}) {
  const [failedUrl, setFailedUrl] = useState<string>();
  const url = profile?.avatar;
  if (url && failedUrl !== url) {
    return (
      <img
        className="account-avatar"
        src={url}
        alt=""
        width={size}
        height={size}
        onError={() => setFailedUrl(url)}
      />
    );
  }
  return (
    <span
      className="account-avatar account-avatar-fallback"
      style={{ width: size, height: size }}
      aria-hidden="true"
    >
      <CircleUserRound size={Math.round(size * 0.75)} />
    </span>
  );
}
