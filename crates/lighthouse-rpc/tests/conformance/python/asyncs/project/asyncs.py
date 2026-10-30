import asyncio


async def fetch(n: int) -> int:
    await asyncio.sleep(0)
    return n


async def gather(count: int):
    results = [await fetch(i) for i in range(count)]
    async for item in stream():
        results.append(item)
    async with lock():
        pass
    return results


async def stream():
    yield 1


def lock():
    return None
