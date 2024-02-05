import asyncio
import json
from mqtt_as import MQTTClient, config
import machine
from time import sleep

from secrets import WIFI_SSID, WIFI_PASSWORD

door_pin = machine.Pin(18, machine.Pin.OUT)
reed_pin = machine.Pin(21, machine.Pin.IN)

# Local configuration
config['ssid'] = WIFI_SSID
config['wifi_pw'] = WIFI_PASSWORD
config['server'] = "10.0.20.179"


async def messages(client):
    async for topic, msg, retained in client.queue:
        if topic.startswith("garagedoor/set"):
            msg = json.loads(msg)
            if msg["open"] and reed_pin.value():
                door_pin.on()
                sleep(1)
                door_pin.off()

            if not msg["open"] and not reed_pin.value():
                door_pin.on()
                sleep(1)
                door_pin.off()

        elif topic.startswith("garagedoor/get"):
            result = {
                "open": 0 if reed_pin.value() else 1
            }
            await client.publish('garagedoor/get/response', json.dumps(result), qos=0)

        elif topic.startswith("garagedoor/reset"):
            machine.reset()

        else:
            print("Unknown MQTT message:", topic, msg, retained)


async def up(client):
    while True:
        await client.up.wait()
        client.up.clear()
        await client.subscribe("garagedoor/+", 0)


async def main(client):
    await client.connect()
    for coroutine in (up, messages):
        asyncio.create_task(coroutine(client))

    while True:
        await asyncio.sleep(5)

config["queue_len"] = 6
MQTTClient.DEBUG = True
_client = MQTTClient(config)
try:
    asyncio.run(main(_client))
finally:
    _client.close()
