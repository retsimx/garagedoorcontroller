import asyncio
import json
from mqtt_as import MQTTClient, config
import machine
from time import sleep_ms

from secrets import WIFI_SSID, WIFI_PASSWORD, MQTT_IP

door_pin = machine.Pin(18, machine.Pin.OUT, value=0)
reed_pin = machine.Pin(21, machine.Pin.IN)

# Local configuration
config['ssid'] = WIFI_SSID
config['wifi_pw'] = WIFI_PASSWORD
config['server'] = MQTT_IP


async def messages(client):
    async for topic, msg, retained in client.queue:
        # Parse the message as JSON to extract UUID
        payload = json.loads(msg.decode())
        request_uuid = payload.get("uuid")

        if topic.startswith("garagedoor/trigger"):
            door_pin.on()
            sleep_ms(600)
            door_pin.off()

        elif topic.startswith("garagedoor/status"):
            result = {
                "open": False if reed_pin.value() else True
            }
            
            response = {
                "uuid": request_uuid,
                "result": result
            }
                
            await client.publish('garagedoor/status/response', json.dumps(response), qos=0)

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

    last_state = None
    while True:
        new_state = reed_pin.value()
        if last_state != new_state:
            last_state = new_state

            result = {
                "open": False if new_state else True
            }

            await client.publish('garagedoor/onchange', json.dumps(result), qos=0)

        await asyncio.sleep(1)

config["queue_len"] = 6
MQTTClient.DEBUG = True
_client = MQTTClient(config)
try:
    asyncio.run(main(_client))
finally:
    _client.close()
