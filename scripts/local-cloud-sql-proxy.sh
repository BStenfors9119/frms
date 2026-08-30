export GOOGLE_APPLICATION_CREDENTIALS=/home/bstenfors/dev/tsr-back-end/auth.json
export INSTANCE_HOST='10.125.32.25'
export DB_PORT='3306'
export DB_NAME='tsr'
export DB_USER='TSRMgr'
export DB_PASS='Zoipdrat324?'

#sudo systemctl stop mysql

#PID=$(pgrep -f cloud-sql-proxy)
#if [ -n "$PID" ]; then
#    echo "Cloud SQL Proxy is already running with PID: $PID"
#    exit 0
#fi

DB_INSTANCE_CONNECTION_NAME=the-sports-remote-272021:us-west2:tsr-8-4
# Start the Cloud SQL Proxy
cd /home/bstenfors/dev/tsr-back-end/
./cloud-sql-proxy --credentials-file $GOOGLE_APPLICATION_CREDENTIALS $DB_INSTANCE_CONNECTION_NAME &
