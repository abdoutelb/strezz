FROM ubuntu:16.04

RUN  apt-get update  && apt-get  install -y apt-transport-https \
 && apt-get  install -y curl \
 && apt-get  install -y python-pip python-dev build-essential \
 &&  pip install locustio 
# Bundle app source


COPY . .

EXPOSE 4000

CMD ["locust","-f","/loc.py","--host=$URL","-P 4000"]
#RUN locust -f locustfile.py --host=$URL -P 3000

#TO buld the images  docker build -t stress-web --build-arg URL=http://www.akhbarak.net/ .