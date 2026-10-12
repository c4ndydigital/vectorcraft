#!/usr/bin/env python3
"""
Empirical Collab & Cursor Verification Harness for VectorCraft.
Authors: Empirical Challenger (critic, specialist)
Tasks verified:
1. Latency of cursor updates over WebSocket (< 300 ms, 20/20 samples).
2. Room isolation: clients in Room A receive ZERO messages from Room B.
3. Nickname updates propagate to peers.
4. Clock skew resilience over WebSocket wire.
5. Adversarial burst throughput & resilience to malformed payloads.
"""

import asyncio
import json
import statistics
import sys
import time
import uuid
import websockets

SERVER_WS = "ws://localhost:8767/ws"

class TestResult:
    def __init__(self):
        self.passes = []
        self.failures = []

    def check(self, condition: bool, name: str, details: str = ""):
        if condition:
            self.passes.append((name, details))
            print(f"  [PASS] {name} {details}")
        else:
            self.failures.append((name, details))
            print(f"  [FAIL] {name} {details}", file=sys.stderr)

async def drain_initial_snapshot(ws, client_name=""):
    """Reads initial binary CRDT snapshot frame sent on connect."""
    msg = await asyncio.wait_for(ws.recv(), timeout=5.0)
    assert isinstance(msg, bytes), f"Expected initial snapshot bytes, got {type(msg)}"
    # print(f"    {client_name} received initial snapshot: {len(msg)} bytes")
    return msg

async def test_cursor_latency(res: TestResult):
    print("\n--- Test 1: Dual-Client Cursor Sync Latency (20/20 samples < 300ms) ---")
    room_id = f"test-cursor-{uuid.uuid4().hex[:8]}"
    uri = f"{SERVER_WS}?room={room_id}"

    async with websockets.connect(uri) as ws_a, websockets.connect(uri) as ws_b:
        await drain_initial_snapshot(ws_a, "Client A")
        await drain_initial_snapshot(ws_b, "Client B")

        latencies = []
        num_samples = 20

        for i in range(num_samples):
            cursor_payload = {
                "client_id": 1001,
                "name": "Peer_A",
                "color": [255, 64, 64, 255],
                "cursor": {"x": round(100.0 + i * 2.5, 1), "y": round(200.0 + i * 1.5, 1)},
                "last_seen": time.time(),
                "seq": i,
            }
            t_send = time.perf_counter()
            await ws_a.send(json.dumps(cursor_payload))

            # Receive on Client B
            received_raw = await asyncio.wait_for(ws_b.recv(), timeout=2.0)
            t_recv = time.perf_counter()
            latency_ms = (t_recv - t_send) * 1000.0
            latencies.append(latency_ms)

            received_data = json.loads(received_raw)
            assert received_data.get("seq") == i, f"Sequence mismatch: expected {i}, got {received_data.get('seq')}"

            # Brief pause between samples
            await asyncio.sleep(0.02)

        min_lat = min(latencies)
        max_lat = max(latencies)
        avg_lat = statistics.mean(latencies)
        p95_lat = sorted(latencies)[int(len(latencies) * 0.95)]
        all_under_300 = all(l < 300.0 for l in latencies)

        print(f"    Samples: {len(latencies)}/{num_samples}")
        print(f"    Min: {min_lat:.2f} ms | Max: {max_lat:.2f} ms | Avg: {avg_lat:.2f} ms | P95: {p95_lat:.2f} ms")

        res.check(len(latencies) == num_samples, "20/20 samples collected", f"({len(latencies)}/20)")
        res.check(all_under_300, "All 20/20 cursor samples < 300 ms", f"(max observed: {max_lat:.2f} ms)")

async def test_room_isolation(res: TestResult):
    print("\n--- Test 2: Room Isolation (Client in Room A sends; Room B receives 0 frames) ---")
    room_a = f"room-alpha-{uuid.uuid4().hex[:8]}"
    room_b = f"room-beta-{uuid.uuid4().hex[:8]}"

    uri_a = f"{SERVER_WS}?room={room_a}"
    uri_b = f"{SERVER_WS}?room={room_b}"

    async with websockets.connect(uri_a) as ws_a1, \
               websockets.connect(uri_a) as ws_a2, \
               websockets.connect(uri_b) as ws_b1:

        await drain_initial_snapshot(ws_a1, "Room A Client 1")
        await drain_initial_snapshot(ws_a2, "Room A Client 2")
        await drain_initial_snapshot(ws_b1, "Room B Client 1")

        # Room B listener: should receive 0 messages
        room_b_messages = []
        async def listen_room_b():
            try:
                while True:
                    msg = await asyncio.wait_for(ws_b1.recv(), timeout=0.5)
                    room_b_messages.append(msg)
            except asyncio.TimeoutError:
                pass

        listener_task = asyncio.create_task(listen_room_b())

        # Send 10 presence frames in Room A
        for i in range(10):
            payload = {
                "client_id": 101,
                "name": f"Alice_{i}",
                "color": [10, 20, 30, 255],
                "cursor": {"x": float(i * 10), "y": float(i * 10)},
                "last_seen": 100.0 + i,
            }
            await ws_a1.send(json.dumps(payload))
            # Confirm Client A2 in Room A receives it
            recvd = await asyncio.wait_for(ws_a2.recv(), timeout=1.0)
            recvd_json = json.loads(recvd)
            assert recvd_json["name"] == f"Alice_{i}"

        # Wait for Room B listener to finish its timeout window
        await listener_task

        res.check(
            len(room_b_messages) == 0,
            "Room Isolation: Room B received 0 frames from Room A",
            f"(received {len(room_b_messages)} frames)"
        )

        # Reverse check: Send from Room B, confirm Room A receives 0 frames
        room_a_extra_messages = []
        async def listen_room_a():
            try:
                while True:
                    msg = await asyncio.wait_for(ws_a2.recv(), timeout=0.5)
                    room_a_extra_messages.append(msg)
            except asyncio.TimeoutError:
                pass

        rev_listener = asyncio.create_task(listen_room_a())

        payload_b = {
            "client_id": 201,
            "name": "Bob_Room_B",
            "color": [50, 60, 70, 255],
            "cursor": {"x": 500.0, "y": 500.0},
            "last_seen": 200.0,
        }
        await ws_b1.send(json.dumps(payload_b))
        await rev_listener

        res.check(
            len(room_a_extra_messages) == 0,
            "Reverse Isolation: Room A received 0 frames from Room B",
            f"(received {len(room_a_extra_messages)} frames)"
        )

async def test_nickname_updates(res: TestResult):
    print("\n--- Test 3: Nickname Updates Propagate to Peers ---")
    room_id = f"test-nick-{uuid.uuid4().hex[:8]}"
    uri = f"{SERVER_WS}?room={room_id}"

    async with websockets.connect(uri) as ws_a, websockets.connect(uri) as ws_b:
        await drain_initial_snapshot(ws_a, "Client A")
        await drain_initial_snapshot(ws_b, "Client B")

        test_names = [
            "Alice",
            "Alice (Team Lead)",
            "Designer-42 🎨",
            "Adrian J.",
            "LongNickNameWithSymbols!@#$%^&*()_+",
        ]

        for name in test_names:
            payload = {
                "client_id": 555,
                "name": name,
                "color": [128, 200, 255, 255],
                "cursor": {"x": 12.3, "y": 45.6},
                "last_seen": 123.456,
            }
            await ws_a.send(json.dumps(payload))
            recvd_raw = await asyncio.wait_for(ws_b.recv(), timeout=2.0)
            recvd = json.loads(recvd_raw)

            res.check(
                recvd.get("name") == name,
                f"Nickname propagated: '{name}'",
                f"(received: '{recvd.get('name')}')"
            )

async def test_clock_skew_payload(res: TestResult):
    print("\n--- Test 4: Clock Skew Wire Transmission ---")
    room_id = f"test-skew-{uuid.uuid4().hex[:8]}"
    uri = f"{SERVER_WS}?room={room_id}"

    async with websockets.connect(uri) as ws_a, websockets.connect(uri) as ws_b:
        await drain_initial_snapshot(ws_a, "Client A")
        await drain_initial_snapshot(ws_b, "Client B")

        # Client A simulates 5.0 seconds uptime (foreign uptime)
        foreign_uptime = 5.0
        payload = {
            "client_id": 999,
            "name": "SkewedUptimeClient",
            "color": [0, 255, 0, 255],
            "cursor": {"x": 150.0, "y": 250.0},
            "last_seen": foreign_uptime,
        }
        await ws_a.send(json.dumps(payload))
        recvd_raw = await asyncio.wait_for(ws_b.recv(), timeout=2.0)
        recvd = json.loads(recvd_raw)

        res.check(
            recvd.get("last_seen") == foreign_uptime,
            "Foreign uptime preserved over wire for client-side local restamping",
            f"(transmitted {foreign_uptime}, received {recvd.get('last_seen')})"
        )

async def test_adversarial_burst_and_resilience(res: TestResult):
    print("\n--- Test 5: Adversarial Stress (High-Rate Burst 50 frames & Malformed Ingestion) ---")
    room_id = f"test-adversarial-{uuid.uuid4().hex[:8]}"
    uri = f"{SERVER_WS}?room={room_id}"

    async with websockets.connect(uri) as ws_a, websockets.connect(uri) as ws_b:
        await drain_initial_snapshot(ws_a, "Client A")
        await drain_initial_snapshot(ws_b, "Client B")

        # 1. Burst flood: 50 messages with no delay
        burst_count = 50
        print(f"    Sending burst of {burst_count} presence packets with zero delay...")
        for i in range(burst_count):
            msg = {
                "client_id": 777,
                "name": "BurstFlooder",
                "color": [100, 100, 100, 255],
                "cursor": {"x": float(i), "y": float(i)},
                "last_seen": time.time(),
                "seq": i,
            }
            await ws_a.send(json.dumps(msg))

        received_count = 0
        try:
            for _ in range(burst_count):
                recvd = await asyncio.wait_for(ws_b.recv(), timeout=3.0)
                recvd_json = json.loads(recvd)
                if recvd_json.get("client_id") == 777:
                    received_count += 1
        except asyncio.TimeoutError:
            pass

        res.check(
            received_count == burst_count,
            f"Burst flood resilience: {received_count}/{burst_count} received without drops",
            f"({received_count}/{burst_count})"
        )

        # 2. Malformed frames: invalid JSON, empty text, truncated payloads
        print("    Injecting malformed payloads into WebSocket...")
        malformed_samples = [
            "{invalid_json",
            "",
            "   ",
            '{"client_id": "not_a_number", "cursor": [1,2,3]}',
            "\x00\x01\x02malformed_utf8_prefix",
        ]
        for bad in malformed_samples:
            await ws_a.send(bad)
            # Server must fan it out as text or drop it, but MUST NOT disconnect or crash
            try:
                _ = await asyncio.wait_for(ws_b.recv(), timeout=0.5)
            except asyncio.TimeoutError:
                pass

        # 3. Post-malformed health check: send a valid frame and verify normal reception
        probe_msg = {
            "client_id": 888,
            "name": "PostAttackRecovery",
            "color": [0, 255, 0, 255],
            "cursor": {"x": 1.0, "y": 1.0},
            "last_seen": time.time(),
        }
        await ws_a.send(json.dumps(probe_msg))
        recvd = await asyncio.wait_for(ws_b.recv(), timeout=2.0)
        recvd_json = json.loads(recvd)
        res.check(
            recvd_json.get("client_id") == 888,
            "Server survived malformed payloads and resumed normal operation",
            f"(probe received: {recvd_json.get('name')})"
        )

async def main():
    print("==================================================================")
    print("  VectorCraft Empirical Collab & Cursor Verification Suite")
    print(f"  Target: {SERVER_WS}")
    print("==================================================================")

    res = TestResult()
    try:
        await test_cursor_latency(res)
        await test_room_isolation(res)
        await test_nickname_updates(res)
        await test_clock_skew_payload(res)
        await test_adversarial_burst_and_resilience(res)
    except Exception as e:
        print(f"\n[FATAL ERROR IN HARNESS]: {e}", file=sys.stderr)
        import traceback
        traceback.print_exc()
        sys.exit(2)

    print("\n==================================================================")
    print(f"  Summary: {len(res.passes)} Passed, {len(res.failures)} Failed")
    print("==================================================================")

    if res.failures:
        print("FAILURES DETECTED:")
        for name, details in res.failures:
            print(f"  - {name}: {details}")
        sys.exit(1)
    else:
        print("ALL VERIFICATION CHECKS PASSED.")
        sys.exit(0)

if __name__ == "__main__":
    asyncio.run(main())
