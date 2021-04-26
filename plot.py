import matplotlib.pyplot as plt

send_sync_id = []
send_timestamp = []
recv_sync_id = []
recv_timestamp = []
with open("./send_sync_data.txt", "r") as f:
    for line in f.readlines():
        line = line.strip().split(':')
        send_sync_id.append(int(line[0]))
        timestamp = float(line[2].strip().split('s')[0]) * 1000
        send_timestamp.append(timestamp)

with open("./send_recv_data.txt", "r") as f:
    for line in f.readlines():
        line = line.strip().split(':')
        recv_sync_id.append(int(line[0]))
        timestamp = float(line[2].strip().split('s')[0]) * 1000
        recv_timestamp.append(timestamp)

min_send_timestamp = min(send_timestamp)
max_send_timestamp = max(send_timestamp)
min_recv_timestamp = min(recv_timestamp)
max_recv_timestamp = max(recv_timestamp)
print("send :" + str(max_send_timestamp - min_send_timestamp) + "ms")
print("recv :" + str(max_recv_timestamp - min_recv_timestamp) + "ms")
plt.plot(send_sync_id, send_timestamp, color="blue")
plt.plot(recv_sync_id, recv_timestamp, color="r")
plt.show()