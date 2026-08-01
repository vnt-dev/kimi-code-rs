export interface AccountMenuVisibility {
  showLogin: boolean;
  showUsage: boolean;
  showSignOut: boolean;
}

export function resolveAccountMenuVisibility(
  loggedIn: boolean,
): AccountMenuVisibility {
  return {
    showLogin: !loggedIn,
    showUsage: loggedIn,
    showSignOut: loggedIn,
  };
}
