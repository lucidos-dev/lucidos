import { Explainer } from '../shared/Explainer';

export function RelayBlockedExplainer({ gateway }: { gateway: string }) {
  return (
    <Explainer title="Relay connection">
      <p>
        You opened this workspace through another gateway, which relays it to "{gateway}".
        Pages and messages are plain requests, so they get through.
        The terminal needs a live connection (a WebSocket), and the relay did not carry it.
      </p>
      <p>The browser only sees the connection close, not why. The usual causes:</p>
      <ul>
        <li>One of the gateways runs an older version that cannot relay live connections.</li>
        <li>"{gateway}" is offline or restarting.</li>
        <li>A proxy or VPN between the gateways drops WebSockets.</li>
      </ul>
      <p>What to do:</p>
      <ul>
        <li>Open this workspace on "{gateway}" directly. Then nothing needs a relay.</li>
        <li>Or update both gateways to the latest version, then tap Try again.</li>
      </ul>
    </Explainer>
  );
}
